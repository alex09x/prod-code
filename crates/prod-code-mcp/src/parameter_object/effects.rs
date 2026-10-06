/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::binding::argument_value;
use super::params::leading_ident;
use super::types::{Language, Param};

/// Whether a JavaScript expression is a constant, the same value wherever and whenever it is
/// evaluated: a number, a string with no substitution, `true`, `false`, `null`, `void 0`, or an
/// empty `[]` or `{}` (a new one either way). A default like that can be written at the call
/// instead of in the function without meaning anything else.
///
/// `undefined` is not one: it is a name, which a parameter or a variable can shadow. Nor is
/// `1..x`, which reads a property of a number and is `undefined` when there is none.
pub fn js_constant(expr: &str) -> bool {
    let e = expr.trim();
    if matches!(e, "true" | "false" | "null" | "void 0" | "[]" | "{}") {
        return true;
    }
    number_literal(e.strip_prefix('-').map_or(e, str::trim_start))
        || (string_literal(e) && !(e.starts_with('`') && e.contains("${")))
}

/// Whether `e` is one number literal and nothing after it: digits with a fraction, an exponent,
/// separators, a radix prefix or a type suffix (`1.5e-3`, `0xff`, `1_000`, `10n`, `2u8`). A point
/// is followed by a digit, an exponent or nothing; a name after it (`1..x`, `1.e`) is a property.
fn number_literal(e: &str) -> bool {
    let b = e.as_bytes();
    let starts = b.first().is_some_and(u8::is_ascii_digit)
        || (b.first() == Some(&b'.') && b.get(1).is_some_and(u8::is_ascii_digit));
    if !starts {
        return false;
    }
    let radix = b.len() > 1 && b[0] == b'0' && b[1].is_ascii_alphabetic();
    let exponent = |at: usize| {
        matches!(b.get(at), Some(b'e' | b'E'))
            && b.get(at + 1)
                .is_some_and(|n| n.is_ascii_digit() || matches!(n, b'+' | b'-'))
    };
    let mut points = 0;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'.' => {
                points += 1;
                let fraction = b.get(i + 1).is_none_or(u8::is_ascii_digit) || exponent(i + 1);
                if points > 1 || radix || !fraction {
                    return false;
                }
            }
            // A sign only in a decimal exponent: in `0xe+1` it is an addition.
            b'+' | b'-' if !radix && i > 0 && matches!(b[i - 1], b'e' | b'E') => {}
            c if c.is_ascii_alphanumeric() || c == b'_' => {}
            _ => return false,
        }
    }
    true
}

/// Whether `e` is one quoted literal: it starts and ends with the same quote, and has no
/// unescaped quote of its kind inside, which would make it two joined. What it may substitute
/// (`${…}`, `\(…)`) is the caller's to rule out.
fn string_literal(e: &str) -> bool {
    let bytes = e.as_bytes();
    let Some(&quote) = bytes.first().filter(|q| matches!(**q, b'"' | b'\'' | b'`')) else {
        return false;
    };
    if bytes.len() < 2 || bytes[bytes.len() - 1] != quote {
        return false;
    }
    let inner = &bytes[1..bytes.len() - 1];
    let mut escaped = false;
    for &b in inner {
        if escaped {
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == quote {
            return false;
        }
    }
    !escaped
}

/// What evaluating an argument may do, as far as its text and the language show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Effect {
    /// A literal converted by the language alone: it reads and writes nothing.
    Nothing,
    /// A plain name in a language where reading one runs nothing: it reads a variable and
    /// writes nothing.
    Reads,
    /// A call, an assignment, a property that may be a getter: anything.
    Unknown,
}

/// Why a plain name does not show that reading it runs nothing, in the languages where it does
/// not (#436); `None` in Rust and Go, where a name is a variable, a constant or a function, and
/// reading one runs no code of the program's.
///
/// In JavaScript `Object.defineProperty(globalThis, "b", { get() { … } })` makes `b` a getter,
/// so `f(a, b, c)` reads `a`, `b`, `c` where `f({ a: a, c: c }, b)` reads `a`, `c`, `b`.
fn opaque_reads(language: Language) -> Option<&'static str> {
    match language {
        Language::Rust | Language::Go | Language::Java => None,
        Language::TypeScript | Language::JavaScript => Some(
            "a plain name may be an accessor of `globalThis`, whose getter runs when it is read",
        ),
        Language::Swift => Some(
            "a plain name may be a computed property, whose getter runs when it is read, and a \
             literal runs the `init(…Literal:)` of a type the program declares",
        ),
        Language::Cpp => Some(
            "a plain name may be a macro, a value passed by copy runs a constructor, and a \
             literal may run a converting constructor or a literal operator",
        ),
        Language::C => Some("a plain name may be a macro"),
        Language::Python => Some(
            "a global or class-level name is looked up through a mapping that may run code, and \
             an unbound one raises for whichever is read first",
        ),
    }
}

/// What evaluating `arg`, passed as `param`, may do.
///
/// A literal is inert only where its conversion to the parameter's type is the language's own:
/// a Swift literal passed as a type the program declares runs that type's `init(…Literal:)`, a
/// C++ one passed as a class runs a converting constructor, and a C++ number or string with a
/// suffix of the program's (`12_km`, `"x"_s`) calls its `operator""`. So in those two languages
/// a literal counts only when the parameter's declared type is a standard one; a parameter the
/// call binds to nothing has none.
fn effect_of(arg: &str, param: Option<&Param>, language: Language) -> Effect {
    let e = arg.trim();
    let literal = match language {
        Language::TypeScript | Language::JavaScript => js_constant(e),
        _ => {
            let word = match language {
                Language::Python => matches!(e, "True" | "False" | "None"),
                Language::Rust => matches!(e, "true" | "false"),
                Language::Swift => matches!(e, "true" | "false" | "nil"),
                Language::Cpp => matches!(e, "true" | "false" | "nullptr"),
                _ => false,
            };
            // C++ separates digits with `'`, so an `_` in a number starts a user-defined suffix.
            let number = number_literal(e.strip_prefix('-').map_or(e, str::trim_start))
                && !(language == Language::Cpp && e.contains('_'));
            word || number || (string_literal(e) && !e.contains("\\(") && !e.starts_with('`'))
        }
    };
    if literal {
        let ty = param.and_then(|p| p.ty.as_deref());
        let inert = match language {
            Language::Swift => ty.is_some_and(swift_literal_type),
            Language::Cpp => param.is_some_and(|p| ty.is_some_and(|t| cpp_builtin(t, &p.name))),
            _ => true,
        };
        return if inert {
            Effect::Nothing
        } else {
            Effect::Unknown
        };
    }
    let name = leading_ident(e);
    if !name.is_empty()
        && name.len() == e.len()
        && !name.as_bytes()[0].is_ascii_digit()
        && !matches!(name, "yield" | "await")
        && opaque_reads(language).is_none()
    {
        return Effect::Reads;
    }
    Effect::Unknown
}

/// Whether a Swift parameter type is one the standard library initialises a literal of,
/// optional or not: a number, `Bool`, `String` or `Character`.
fn swift_literal_type(ty: &str) -> bool {
    let ty = ty.trim().trim_end_matches(['?', '!']);
    let ty = ty.strip_prefix("Swift.").unwrap_or(ty);
    matches!(
        ty,
        "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Double"
            | "Float"
            | "Bool"
            | "String"
            | "Character"
    )
}

/// Whether a C++ parameter, declared as `ty` (its name `name` included), has a built-in type —
/// arithmetic, `bool`, a character, or a pointer or reference to one — which a literal
/// converts to without a constructor.
pub(crate) fn cpp_builtin(ty: &str, name: &str) -> bool {
    const BUILTIN: &[&str] = &[
        "const", "volatile", "signed", "unsigned", "short", "long", "int", "char", "char8_t",
        "char16_t", "char32_t", "wchar_t", "bool", "float", "double",
    ];
    let spaced = ty.replace(['*', '&'], " ");
    let words: Vec<&str> = spaced.split_whitespace().filter(|w| *w != name).collect();
    !words.is_empty() && words.iter().all(|w| BUILTIN.contains(w))
}

/// Two arguments of a call whose order of evaluation bundling would change, the bundled one
/// first, when nothing shows that it does not matter (#436). The literal is written where the
/// first bundled argument was, holding the bundled arguments in the order the call wrote them,
/// so a bundled argument after one that is not bundled is evaluated before it afterwards. That
/// is the same program when either of the two is a literal the language converts by itself, or
/// when both are plain names in a language where reading a variable runs nothing — Rust and Go
/// (see [`opaque_reads`]). Anything else is refused: the spelling of a name is no proof.
///
/// `params` are the declared parameters `bound` indexes, whose types say how a literal is
/// converted; Rust's path, whose literals need no type, passes none.
pub fn reordered_arguments<'a>(
    args: &'a [String],
    bound: &[Option<usize>],
    bundled: &[usize],
    params: &[Param],
    language: Language,
) -> Option<(&'a str, &'a str)> {
    let in_bundle = |a: usize| bound[a].is_some_and(|p| bundled.contains(&p));
    let first = (0..args.len()).find(|a| in_bundle(*a))?;
    let effect = |a: usize| {
        let param = bound[a].and_then(|p| params.get(p));
        effect_of(argument_value(&args[a], language), param, language)
    };
    for passed in (first..args.len()).filter(|a| !in_bundle(*a)) {
        for moved in (passed + 1..args.len()).filter(|a| in_bundle(*a)) {
            let commute = matches!(
                (effect(moved), effect(passed)),
                (Effect::Nothing, _) | (_, Effect::Nothing) | (Effect::Reads, Effect::Reads)
            );
            if !commute {
                return Some((args[moved].trim(), args[passed].trim()));
            }
        }
    }
    None
}

/// The refusal for a call whose arguments bundling would evaluate in another order; see
/// [`reordered_arguments`].
pub(crate) fn reordered(
    callee: &str,
    place: &str,
    moved: &str,
    passed: &str,
    language: Language,
) -> anyhow::Error {
    let (why, advice) = match opaque_reads(language) {
        Some(why) => (
            format!(" (in {}, {why})", language.label()),
            "Bundle adjacent parameters",
        ),
        None => (
            String::new(),
            "Bundle adjacent parameters, or give the arguments names first",
        ),
    };
    anyhow::anyhow!(
        "`{callee}` at {place} passes `{passed}` between the bundled arguments; in the object \
         `{moved}` would be evaluated before it, and either may change what the other \
         sees{why}. {advice}; nothing was rewritten"
    )
}
