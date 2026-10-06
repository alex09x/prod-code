/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::binding::{argument_value, keyword_arg};
use super::params::leading_ident;
use super::syntax::{close_in, is_ident_byte, walk_code};
use super::type_render::literal_text;
use super::types::{Language, Param};

/// The argument list a call should end up with: the bundled arguments collected into one
/// struct literal, in the position the first of them had, everything else where it was.
///
/// `spelling` is how the type is named *in this file* — its bare name where an import can
/// carry it, its full path where nothing can (a file under `tests/` is not a module of the
/// crate and cannot import from `crate::`).
pub fn rewritten_args(
    args: &[String],
    bundled: &[usize],
    spelling: &str,
    fields: &[(String, String)],
) -> String {
    let literal = {
        let inner: Vec<String> = bundled
            .iter()
            .enumerate()
            .map(|(n, arg)| {
                // A variable of the field's name is passed in shorthand, as clippy's
                // `redundant_field_names` wants it (#342).
                let (field, value) = (&fields[n].0, args[*arg].trim());
                if value == field {
                    field.clone()
                } else {
                    format!("{field}: {value}")
                }
            })
            .collect();
        format!("{spelling} {{ {} }}", inner.join(", "))
    };
    let first = bundled.first().copied().unwrap_or(0);
    let mut out = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if i == first {
            out.push(literal.clone());
        } else if !bundled.contains(&i) {
            out.push(arg.clone());
        }
    }
    out.join(", ")
}

/// The argument list a call ends up with: the bundled arguments collected into one literal where
/// the first of them was, every other argument where it was.
///
/// In Python the literal is passed the way that first argument was: by keyword when it was a
/// keyword argument, since a positional argument cannot follow one. A bundled parameter the call
/// left to its default is left out of the literal, where the field's default stands in for it;
/// when the call passed none of them, the literal goes last, by keyword.
pub fn rewritten_call(
    args: &[String],
    bound: &[Option<usize>],
    bundled: &[usize],
    params: &[Param],
    language: Language,
    spelling: &str,
    binding: &str,
) -> String {
    rewritten_call_with(args, bound, bundled, params, language, binding, |pairs| {
        literal_text(language, spelling, pairs)
    })
}

/// [`rewritten_call`] with the literal written by `literal`, for a language where it depends on
/// more than the language — a C++ project before C++20 has no designators.
///
/// A Swift argument is passed under the new parameter's label, which it has unless the first
/// bundled parameter had none. When a Swift or C++ call passed none of the bundled arguments,
/// every one of them had a default, and so does the new parameter; the call stays as it was.
pub(crate) fn rewritten_call_with(
    args: &[String],
    bound: &[Option<usize>],
    bundled: &[usize],
    params: &[Param],
    language: Language,
    binding: &str,
    literal: impl Fn(&[(String, String)]) -> String,
) -> String {
    // TypeScript applies a default when the argument is `undefined` the way JavaScript does, and
    // only a constant one gets this far in either.
    let js = matches!(language, Language::JavaScript | Language::TypeScript);
    // In the order the call wrote them, which is the order they are evaluated in: a Python
    // call may pass keywords in any order (#436). A field the call left out goes after them.
    let mut pairs: Vec<(usize, (String, String))> = bundled
        .iter()
        .filter_map(|p| {
            let name = params[*p].name.clone();
            // A JavaScript default stands in for `undefined`, passed or left out; the object
            // carries it now, since the field has none. Only a constant gets this far.
            let default = params[*p].default.clone().filter(|_| js);
            match bound.iter().position(|b| *b == Some(*p)) {
                Some(a) if default.is_some() && args[a].trim() == "void 0" => {
                    Some((a, (name, default?)))
                }
                Some(a) => Some((a, (name, argument_value(&args[a], language).to_string()))),
                // Left out, a JavaScript parameter was `undefined`, and the field is too — an own
                // one, so that `toString` does not read the one every object inherits. `void 0`
                // is `undefined` wherever it is written; the name can be shadowed.
                None if js => Some((
                    usize::MAX,
                    (name, default.unwrap_or_else(|| "void 0".to_string())),
                )),
                None => None,
            }
        })
        .collect();
    pairs.sort_by_key(|(at, _)| *at);
    let pairs: Vec<(String, String)> = pairs.into_iter().map(|(_, pair)| pair).collect();
    let literal = literal(&pairs);
    let first = bound
        .iter()
        .position(|b| b.is_some_and(|p| bundled.contains(&p)));
    let labelled =
        language == Language::Swift && bundled.first().is_some_and(|p| params[*p].label.is_some());
    let mut out = Vec::new();
    for (a, arg) in args.iter().enumerate() {
        if Some(a) == first {
            if language == Language::Python && keyword_arg(arg).is_some() {
                out.push(format!("{binding}={literal}"));
            } else if labelled {
                out.push(format!("{binding}: {literal}"));
            } else {
                out.push(literal.clone());
            }
        } else if !bound[a].is_some_and(|p| bundled.contains(&p)) {
            out.push(arg.clone());
        }
    }
    if first.is_none() && language == Language::Python {
        out.push(format!("{binding}={literal}"));
    }
    // A JavaScript call that stopped short of the bundled parameters still has to pass the
    // object, which the body reads fields of; what it left out before it was `undefined`.
    if first.is_none() && js {
        let at = bundled.first().copied().unwrap_or(0);
        while out.len() < at {
            out.push("void 0".to_string());
        }
        out.push(literal);
    }
    out.join(", ")
}

/// How long the name at a reference the analyzer reported is: the function's own, or in
/// JavaScript one an import gave it (`import { build as make }`, `const { build: make } =
/// require(…)`, a default import). `None` when neither is written there: the file changed since
/// the analyzer read it, and the position is not trusted (#75).
pub(crate) fn called_name(
    text: &str,
    at: usize,
    callee: &str,
    language: Language,
) -> Option<usize> {
    let rest = &text[at..];
    if language != Language::JavaScript {
        return rest.starts_with(callee).then_some(callee.len());
    }
    let name = leading_ident(rest);
    (!name.is_empty() && (name == callee || js_alias(text, callee, name))).then_some(name.len())
}

/// Whether a JavaScript file gives `callee` the local name `alias`: renamed in an import or a
/// destructured `require`, or bound by a default import or a whole-module `require`.
fn js_alias(text: &str, callee: &str, alias: &str) -> bool {
    let bytes = text.as_bytes();
    let word_at = |at: usize, word: &str| {
        (at == 0 || !is_ident_byte(bytes[at - 1]))
            && !bytes
                .get(at + word.len())
                .is_some_and(|b| is_ident_byte(*b))
    };
    for (at, _) in text.match_indices(callee) {
        if !word_at(at, callee) {
            continue;
        }
        let after = text[at + callee.len()..].trim_start();
        let renamed = after
            .strip_prefix("as")
            .filter(|r| r.starts_with(char::is_whitespace))
            .or_else(|| after.strip_prefix(':').filter(|r| !r.starts_with(':')));
        if renamed.is_some_and(|r| leading_ident(r.trim_start()) == alias) {
            return true;
        }
    }
    for (at, _) in text.match_indices(alias) {
        if !word_at(at, alias) {
            continue;
        }
        let before = text[..at].trim_end();
        let after = text[at + alias.len()..].trim_start();
        let keyword = |k: &str| {
            before.ends_with(k)
                && !before[..before.len() - k.len()]
                    .bytes()
                    .next_back()
                    .is_some_and(is_ident_byte)
        };
        if keyword("import") && (after.starts_with("from") || after.starts_with(',')) {
            return true;
        }
        if (keyword("const") || keyword("let") || keyword("var"))
            && after
                .strip_prefix('=')
                .is_some_and(|r| r.trim_start().starts_with("require("))
        {
            return true;
        }
    }
    false
}

/// Where `word` is written as a name between `from` and `to` of a JavaScript text: in code and
/// in the `${…}` of a template literal, not in a string or a comment, not as a property after a
/// `.` and not as the key of an object literal.
pub(crate) fn ident_uses(text: &str, from: usize, to: usize, word: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let is_use = |at: usize| {
        // An ASCII byte is where a character starts, so the slices below are safe.
        if !is_ident_byte(bytes[at])
            || !text[at..].starts_with(word)
            || (at > 0 && is_ident_byte(bytes[at - 1]))
            || bytes
                .get(at + word.len())
                .is_some_and(|b| is_ident_byte(*b))
        {
            return false;
        }
        let before = text[..at].trim_end();
        let after = text[at + word.len()..].trim_start();
        let property = before.ends_with('.') && !before.ends_with("...");
        let key = (before.ends_with('{') || before.ends_with(','))
            && after.starts_with(':')
            && !after.starts_with("::");
        !property && !key
    };
    let mut out = Vec::new();
    let mut template: Option<usize> = None;
    walk_code(text, from, Language::JavaScript, |i, c| {
        if i >= to {
            return false;
        }
        if c == b'`' {
            let Some(open) = template.take() else {
                template = Some(i);
                return true;
            };
            let mut at = open + 1;
            while let Some(n) = text[at..i].find("${") {
                let start = at + n + 1;
                let end = close_in(text, start, Language::JavaScript).map_or(i, |e| e.min(i));
                out.extend((start + 1..end).filter(|o| is_use(*o)));
                at = end;
            }
            return true;
        }
        if is_ident_byte(c) && is_use(i) {
            out.push(i);
        }
        true
    });
    out
}

/// Whether the name at `at` is a shorthand property of an object literal (`{ width }`) or of a
/// destructuring assignment (`({ width } = o)`), which becomes `width: size.width` — a bare
/// `size.width` there does not parse. The braces around it tell an object from a block and from
/// a JSX expression: an object follows an operator, an opening bracket or `return`; a block
/// follows `)`, `=>` or a keyword, and a JSX expression an attribute's `=` or a tag's `>`.
pub(crate) fn object_shorthand(text: &str, from: usize, at: usize, len: usize) -> bool {
    let before = text[..at].trim_end();
    let after = text[at + len..].trim_start();
    let ends_an_entry = after.starts_with([',', '}'])
        || (after.starts_with('=') && !after.starts_with("==") && !after.starts_with("=>"));
    if !(before.ends_with('{') || before.ends_with(',')) || !ends_an_entry {
        return false;
    }
    let mut open: Vec<usize> = Vec::new();
    let mut in_string = false;
    walk_code(text, from, Language::JavaScript, |i, c| {
        if i >= at {
            return false;
        }
        match c {
            b'"' | b'\'' | b'`' => in_string = !in_string,
            b'(' | b'[' | b'{' => open.push(i),
            b')' | b']' | b'}' => {
                open.pop();
            }
            _ => {}
        }
        true
    });
    let Some(&brace) = open.last() else {
        return false;
    };
    if in_string || text.as_bytes()[brace] != b'{' {
        return false;
    }
    let lead = text[..brace].trim_end();
    let keyword = |k: &str| {
        lead.ends_with(k)
            && !lead[..lead.len() - k.len()]
                .bytes()
                .next_back()
                .is_some_and(is_ident_byte)
    };
    match lead.bytes().next_back() {
        Some(b'(' | b'[' | b',' | b':' | b'?' | b'{' | b'!' | b'&' | b'|') => true,
        // `x = {` is an assignment; `width={` with nothing around the `=` is a JSX attribute.
        Some(b'=') => {
            lead.len() < brace
                || lead[..lead.len() - 1]
                    .bytes()
                    .next_back()
                    .is_some_and(|b| b.is_ascii_whitespace())
        }
        _ => keyword("return") || keyword("yield") || keyword("await"),
    }
}
