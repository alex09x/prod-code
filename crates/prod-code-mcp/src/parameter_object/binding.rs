/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::params::leading_ident;
use super::syntax::{close_in, matching_bracket};
use super::types::{Kind, Language, Param};

/// The arguments of the call whose callee name ends at `after_name`, as a byte range inside
/// the parentheses, or `None` when what follows the name is not a call.
///
/// Brackets nest and string and character literals are skipped, so an argument that is a
/// closure, a method chain or a string containing a comma survives intact.
pub fn call_args_span(text: &str, after_name: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = after_name;
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    if bytes.get(i) != Some(&b'(') {
        return None;
    }
    matching_bracket(text, i).map(|close| (i + 1, close))
}

/// The arguments of a call whose callee name ends at `after_name`, as a byte range inside the
/// parentheses; `None` when what follows the name is not a call. Explicit type arguments in
/// TypeScript and C++ (`build<T>(…)`) come between the name and the list.
pub(crate) fn call_args_in(
    text: &str,
    after_name: usize,
    language: Language,
) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = after_name;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if matches!(language, Language::TypeScript | Language::Cpp) && bytes.get(i) == Some(&b'<') {
        let mut depth = 0i32;
        while i < bytes.len() {
            match bytes[i] {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                b';' | b'\n' => return None,
                _ => {}
            }
            i += 1;
        }
        i += 1;
    }
    if bytes.get(i) != Some(&b'(') {
        return None;
    }
    close_in(text, i, language).map(|close| (i + 1, close))
}

/// A Python keyword argument, as its name and its value: `height=2`, but not `a == b`.
pub(crate) fn keyword_arg(arg: &str) -> Option<(&str, &str)> {
    let name = leading_ident(arg);
    if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let value = arg[name.len()..].trim_start().strip_prefix('=')?;
    if value.starts_with('=') {
        return None;
    }
    Some((name, value.trim()))
}

/// Which parameter each argument of a call binds to, or `None` when the call is not one of this
/// declaration with the right arity. An argument bound to no parameter (`None` in the list) is
/// one Python's `*args` or `**kwargs` takes, and it stays as it was.
///
/// Go passes every argument by position, so the count has to match, as in Rust. TypeScript does
/// too, but a call may leave off trailing parameters that are optional or have defaults.
/// Python binds positional arguments in order and keyword arguments by name; a parameter no
/// argument binds has to have a default. A call that spreads (`*xs`, `**kw`) cannot be mapped
/// without running it.
///
/// C and C++ pass by position too, but a C++ call may leave off trailing parameters that have
/// defaults, and a C `...` takes whatever is left. Swift binds by argument label, in declaration
/// order, and a parameter with a default may be skipped.
pub fn bind_arguments(
    args: &[String],
    params: &[Param],
    language: Language,
) -> Option<Vec<Option<usize>>> {
    match language {
        Language::Python => {}
        Language::Swift => return bind_swift_arguments(args, params),
        Language::JavaScript => return bind_js_arguments(args, params),
        // A parameter left off at the end is `undefined`, which an optional or a defaulted one
        // (or a rest one, as none) accepts. A spread has no position until the call runs.
        Language::TypeScript => {
            let short = params[args.len().min(params.len())..]
                .iter()
                .any(|p| p.default.is_none() && !p.optional && p.kind != Kind::Variadic);
            if args.len() > params.len() || short || args.iter().any(|a| a.starts_with("...")) {
                return None;
            }
            return Some((0..args.len()).map(Some).collect());
        }
        Language::C | Language::Cpp => {
            let variadic = params.last().is_some_and(|p| p.kind == Kind::Variadic);
            let fixed = params.len() - usize::from(variadic);
            if args.len() > fixed && !variadic {
                return None;
            }
            let left_off = &params[args.len().min(fixed)..fixed];
            if left_off.iter().any(|p| p.default.is_none()) {
                return None;
            }
            return Some((0..args.len()).map(|a| Some(a.min(fixed))).collect());
        }
        _ => {
            return (args.len() == params.len()).then(|| (0..args.len()).map(Some).collect());
        }
    }
    let mut positional = Vec::new();
    for (i, p) in params.iter().enumerate() {
        match p.kind {
            Kind::Plain => positional.push(i),
            Kind::Marker if p.name == "*" => break,
            Kind::Variadic | Kind::Keywords => break,
            Kind::Marker => {}
        }
    }
    let takes_rest = params.iter().any(|p| p.kind == Kind::Variadic);
    let takes_keywords = params.iter().any(|p| p.kind == Kind::Keywords);
    let mut taken = vec![false; params.len()];
    let mut bound = Vec::with_capacity(args.len());
    let mut next = 0usize;
    for arg in args {
        if arg.starts_with('*') {
            return None;
        }
        if let Some((key, _)) = keyword_arg(arg) {
            match params
                .iter()
                .position(|p| p.kind == Kind::Plain && p.name == key)
            {
                Some(i) if !taken[i] => {
                    taken[i] = true;
                    bound.push(Some(i));
                }
                None if takes_keywords => bound.push(None),
                _ => return None,
            }
        } else {
            match positional.get(next) {
                Some(&i) => {
                    taken[i] = true;
                    bound.push(Some(i));
                    next += 1;
                }
                None if takes_rest => bound.push(None),
                None => return None,
            }
        }
    }
    let missing = params
        .iter()
        .enumerate()
        .any(|(i, p)| p.kind == Kind::Plain && !taken[i] && p.default.is_none());
    (!missing).then_some(bound)
}

/// A Swift argument label and the value after it: `width: 3`, but not `a ? b : c`.
pub(crate) fn swift_label(arg: &str) -> Option<(&str, &str)> {
    let name = leading_ident(arg);
    if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let value = arg[name.len()..].trim_start().strip_prefix(':')?;
    Some((name, value.trim()))
}

/// Swift's binding: each argument goes to the next parameter whose label it carries, and a
/// parameter passed over has to have a default. The arguments after the first one of a variadic
/// parameter carry no label and belong to it too.
fn bind_swift_arguments(args: &[String], params: &[Param]) -> Option<Vec<Option<usize>>> {
    let mut bound: Vec<Option<usize>> = Vec::with_capacity(args.len());
    let mut next = 0usize;
    for arg in args {
        let label = swift_label(arg).map(|(l, _)| l);
        loop {
            let p = params.get(next)?;
            let fed = bound.last() == Some(&Some(next));
            if p.kind == Kind::Variadic && fed && label.is_none() {
                bound.push(Some(next));
                break;
            }
            if !fed && p.label.as_deref() == label {
                bound.push(Some(next));
                if p.kind != Kind::Variadic {
                    next += 1;
                }
                break;
            }
            // A variadic parameter may be given nothing at all.
            if p.default.is_none() && p.kind != Kind::Variadic && !fed {
                return None;
            }
            next += 1;
        }
    }
    let missing = params
        .iter()
        .enumerate()
        .any(|(i, p)| p.default.is_none() && p.kind != Kind::Variadic && !bound.contains(&Some(i)));
    (!missing).then_some(bound)
}

/// JavaScript's binding: by position, with no count to match. A parameter no argument reaches
/// is `undefined` (or its default), and an argument past the last one binds to nothing — a rest
/// parameter takes it, or nothing does — and stays where it is. A spread argument has no
/// position until the call runs, so a call with one is not bound.
fn bind_js_arguments(args: &[String], params: &[Param]) -> Option<Vec<Option<usize>>> {
    if args.iter().any(|a| a.starts_with("...")) {
        return None;
    }
    let fixed = params
        .iter()
        .take_while(|p| p.kind != Kind::Variadic)
        .count();
    Some((0..args.len()).map(|a| (a < fixed).then_some(a)).collect())
}

/// The value an argument passes, without the keyword (Python) or the label (Swift) it is
/// passed under.
pub(crate) fn argument_value<'a>(arg: &'a str, language: Language) -> &'a str {
    match language {
        Language::Python => keyword_arg(arg).map_or(arg, |(_, v)| v),
        Language::Swift => swift_label(arg).map_or(arg, |(_, v)| v),
        _ => arg,
    }
}
