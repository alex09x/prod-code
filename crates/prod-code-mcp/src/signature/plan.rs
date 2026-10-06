/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};

use crate::signature::call_sites::opaque_at;
use crate::signature::types::{Declared, Param, Plan};

/// Works out the new parameter list, the argument order at the call sites, and what is dropped.
pub fn plan(declared: &[Declared], request: &[Param]) -> Result<Plan> {
    let mut list = Vec::new();
    let mut args = Vec::new();
    let mut kept = Vec::new();
    for want in request {
        match want {
            Param::Keep(name) => {
                let at = declared
                    .iter()
                    .position(|d| &d.name == name)
                    .with_context(|| {
                        format!(
                            "no parameter named `{name}`; the declaration takes {}",
                            declared
                                .iter()
                                .map(|d| d.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })?;
                anyhow::ensure!(!kept.contains(&at), "`{name}` is listed twice");
                kept.push(at);
                list.push(declared[at].raw.clone());
                args.push(Some(at));
            }
            Param::Add { name, ty, .. } => {
                list.push(format!("{name}: {ty}"));
                args.push(None);
            }
        }
    }
    let dropped = declared
        .iter()
        .enumerate()
        .filter(|(i, _)| !kept.contains(i))
        .map(|(_, d)| d.name.clone())
        .collect();
    Ok(Plan {
        list,
        args,
        dropped,
    })
}

/// Formats the new parameter list the way the old one was written: on one line, or one
/// parameter per line with the original indentation.
pub fn format_list(old_inner: &str, receiver: Option<&str>, params: &[String]) -> String {
    let mut all: Vec<String> = Vec::new();
    if let Some(r) = receiver {
        all.push(r.trim().to_string());
    }
    all.extend(params.iter().cloned());
    if !old_inner.contains('\n') {
        return all.join(", ");
    }
    let indent = old_inner
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_else(|| "    ".to_string());
    let closing: String = indent.chars().skip(4).collect();
    let mut out = String::from("\n");
    for p in &all {
        out.push_str(&indent);
        out.push_str(p.trim());
        out.push_str(",\n");
    }
    out.push_str(&closing);
    out
}

/// The structural rule that rewrites the call sites: a placeholder per declared argument, and
/// the replacement in the requested order with the added expressions spelled out.
pub fn call_site_rule(
    name: &str,
    is_method: bool,
    arity: usize,
    args: &[Option<usize>],
    adds: &[&str],
) -> String {
    let pattern_args: Vec<String> = (0..arity).map(|i| format!("${{a{i}}}")).collect();
    let mut added = adds.iter();
    let replacement_args: Vec<String> = args
        .iter()
        .map(|a| match a {
            Some(i) => format!("${{a{i}}}"),
            None => added
                .next()
                .copied()
                .unwrap_or("Default::default()")
                .to_string(),
        })
        .collect();
    // rust-analyzer's placeholders are `$name`, without braces; the braces above only keep the
    // numbering readable while the lists are built.
    let clean = |v: Vec<String>| {
        v.into_iter()
            .map(|s| s.replace("${", "$").replace('}', ""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (pattern, replacement) = (clean(pattern_args), clean(replacement_args));
    if is_method {
        format!("$recv.{name}({pattern}) ==>> $recv.{name}({replacement})")
    } else {
        format!("{name}({pattern}) ==>> {name}({replacement})")
    }
}

pub const AWAIT: &str = ".await";

/// Whether `name` is at byte `at` of `text` as a whole identifier.
pub fn names_at(text: &str, at: usize, name: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    text.is_char_boundary(at)
        && text[at..].starts_with(name)
        && !text[at + name.len()..].starts_with(ident)
        && !text[..at].ends_with(ident)
}

/// Whether the name at `at` is in a `use` item or a comment: it names the function after the
/// change as it did before, and there is nothing to rewrite. `code` is `text` through
/// [`blank_comments`].
pub fn in_use_or_comment(text: &str, code: Option<&str>, at: usize) -> bool {
    if code.is_some_and(|code| code.as_bytes().get(at) == Some(&b' ')) {
        return true;
    }
    // Walk the possible use-tree prefix, including raw identifiers. A keyword is a whole
    // token: the `use` in `r#use` is an identifier, even in `take(r#use, callback)`.
    let source = code.unwrap_or(text);
    let tree = source[..at].trim_end_matches(|c: char| {
        c.is_alphanumeric()
            || c.is_whitespace()
            || matches!(c, '_' | '#' | ':' | '{' | '}' | ',' | '*')
    });
    source[tree.len()..at].match_indices("use").any(|(i, _)| {
        let i = tree.len() + i;
        names_at(source, i, "use") && !source[..i].ends_with('#')
    })
}

/// Where the call whose callee's name is at `at` starts, as far as a rewrite may respell it: the
/// `.` of a method call with the whitespace before it, or the path in front of the name.
pub fn call_start(text: &str, at: usize) -> usize {
    let before = text[..at].trim_end();
    if let Some(dot) = before.strip_suffix('.')
        && !dot.ends_with('.')
    {
        return dot.trim_end().len();
    }
    let mut start = at;
    while let Some(qualifier) = text[..start].trim_end().strip_suffix("::") {
        let qualifier = qualifier.trim_end();
        let segment = qualifier.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_');
        if segment.len() == qualifier.len() {
            break;
        }
        start = segment.len();
    }
    start
}

/// Just past the `)` that closes the argument list opening at `open`, and `None` when it does
/// not close. Brackets inside literals and comments do not count.
pub fn argument_list_end(text: &str, open: usize) -> Option<usize> {
    let s = text.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    while i < s.len() {
        if let Some((end, _)) = opaque_at(text, i, open).ok()? {
            i = end;
            continue;
        }
        match s[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return (s[i] == b')').then_some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The byte length of what `a` and `b` start with alike, at a character boundary of both.
pub fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or_else(|| a.len().min(b.len()), |((i, _), _)| i)
}

/// Whether a call passing `call` comes back from the reorder `plan` exactly as it was: every
/// argument moves to a place that held the same one. Such a call needs no rewrite.
pub fn reorders_to_itself(call: &[String], plan: &[Option<usize>]) -> bool {
    call.len() == plan.len()
        && plan.iter().enumerate().all(|(j, from)| {
            from.and_then(|i| call.get(i))
                .is_some_and(|arg| arg.trim() == call[j].trim())
        })
}
