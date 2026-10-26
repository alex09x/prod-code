/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::extract_field::types::Braces;
use crate::fixture::builder::lexer::{Kind, Source};

mod targets;

/// Preserve byte offsets while hiding comments and literal contents from structural scans.
pub(super) fn code(text: &str) -> Option<String> {
    let source = Source::new(text).ok()?;
    let mut bytes = vec![b' '; text.len()];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            bytes[i] = b;
        }
    }
    for token in source.tokens.iter().filter(|t| t.kind != Kind::Literal) {
        bytes[token.start..token.end].copy_from_slice(&text.as_bytes()[token.start..token.end]);
    }
    String::from_utf8(bytes).ok()
}

pub(super) fn close(text: &str, open: usize) -> Option<usize> {
    let source = Source::new(text).ok()?;
    let index = source.tokens.iter().position(|t| t.start == open)?;
    Some(source.tokens[source.close_of(index)?].start)
}

/// None means bounded classification could not prove a literal or a pattern. In particular,
/// reaching the enclosing-group limit must never turn a possible pattern into a constructor.
pub(super) fn classify(text: &str, open: usize, close: usize) -> Option<Braces> {
    let s = Source::new(text).ok()?;
    let begin = s.tokens.iter().position(|t| t.start == open)?;
    let end = s.tokens.iter().position(|t| t.start == close)?;
    let mut field = begin + 1;
    let mut i = field;
    let mut rest = false;
    while i <= end {
        if i == end || s.is(i, ",") {
            // A rest is an entire field, not the suffix of `range: 0..` or a nested value.
            rest |= i == field + 2 && s.is(field, ".") && s.is(field + 1, ".");
            field = i + 1;
        } else if s.is(i, "(") || s.is(i, "[") || s.is(i, "{") {
            i = s.close_of(i)?;
        }
        i += 1;
    }
    if rest {
        return Some(Braces::Pattern { rest: true });
    }
    pattern_cue(&s, end + 1).map(|pattern| {
        if pattern {
            Braces::Pattern { rest: false }
        } else {
            Braces::Literal
        }
    })
}

pub(super) fn pattern_cue(s: &Source<'_>, mut at: usize) -> Option<bool> {
    // A sibling closure cannot enclose the candidate, even when its closing bar
    // is encountered while looking beyond a comma or an enclosing group.
    let origin = at;
    for depth in 0..=8 {
        // `|` is also a value operator. Resolve its enclosing context before treating
        // an operand as an alternative pattern: `64 | 1;` versus `64 | 65 =>`.
        // Consuming this continuation uses the same bound as enclosing groups.
        if s.is(at, "|") && !s.is(at + 1, "|") {
            if closure_open_bounded(s, at, 8, origin).ok()?.is_some() {
                return Some(true);
            }
            if depth == 8 {
                return None;
            }
            at = after_bar(s, at + 1)?;
            continue;
        }
        if s.is(at, "=>")
            || s.is(at, ":")
            || s.is(at, "in")
            || s.is(at, "if")
            || (s.is(at, "=") && !s.is(at + 1, "="))
        {
            return Some(true);
        }
        if !(s.is(at, ")") || s.is(at, "]") || s.is(at, "}") || s.is(at, ",")) {
            return Some(false);
        }
        if depth == 8 {
            return None;
        }
        // Skip siblings and their groups using the same lexer as the rest of the scan.
        while at < s.tokens.len() {
            if s.is(at, "|") && closure_open_bounded(s, at, 8, origin).ok()?.is_some() {
                return Some(true);
            }
            if s.is(at, ")") || s.is(at, "]") || s.is(at, "}") {
                break;
            }
            if s.is(at, "(") || s.is(at, "[") || s.is(at, "{") {
                at = s.close_of(at)?;
            }
            at += 1;
        }
        if at == s.tokens.len() {
            return Some(false);
        }
        at += 1;
    }
    None
}

/// Prove that a bar closes closure parameters, rather than a bitwise/logical operand
/// or an alternative pattern. Balanced parameter patterns/types are opaque here.
/// An exhausted scan is uncertainty, not evidence of a value operator.
pub(super) fn closure_open(s: &Source<'_>, close: usize) -> Result<Option<usize>, ()> {
    closure_open_bounded(s, close, 8, usize::MAX)
}

fn closure_open_bounded(
    s: &Source<'_>,
    close: usize,
    budget: usize,
    origin: usize,
) -> Result<Option<usize>, ()> {
    if budget == 0 {
        return Err(());
    }
    let mut at = close;
    for _ in 0..256 {
        let Some(previous) = at.checked_sub(1) else {
            return Ok(None);
        };
        at = previous;
        match s.t(at) {
            "|" => {
                // Do not spend the candidate's uncertainty budget recursively
                // classifying a closure that begins in a later sibling.
                if at >= origin {
                    return Ok(None);
                }
                let before = at.checked_sub(1).map(|i| s.t(i));
                // A closure body may itself be another closure, including after `||`.
                if before == Some("|") {
                    return Ok(closure_open_bounded(s, at - 1, budget - 1, origin)?.map(|_| at));
                }
                return Ok(matches!(
                    before,
                    None | Some(
                        "=" | "("
                            | "["
                            | "{"
                            | ","
                            | ";"
                            | "=>"
                            | ":"
                            | "move"
                            | "async"
                            | "static"
                            | "return"
                            | "break"
                            | "&"
                            | "mut"
                            | "!"
                            | "*"
                            | "-"
                    )
                )
                .then_some(at));
            }
            ")" | "]" | "}" => at = s.open_of(at).ok_or(())?,
            "=" | ";" | "=>" | "(" | "[" | "{" => return Ok(None),
            _ => {}
        }
    }
    Err(())
}

/// Find the context after a possible alternative/value continuation, without parsing
/// its expression. Balanced groups and their opaque literals/comments come from Source.
/// A guard's `if` and a value's `if` are ambiguous here; their eventual arm arrow or
/// expression boundary decides. Exhausting the bounded scan remains unknown.
fn after_bar(s: &Source<'_>, mut at: usize) -> Option<usize> {
    for _ in 0..256 {
        if at >= s.tokens.len()
            || matches!(s.t(at), ")" | "]" | "}" | "," | ";" | "=>" | ":" | "in")
            || (s.is(at, "|") && !s.is(at + 1, "|"))
            || (s.is(at, "=")
                && !s.is(at + 1, "=")
                && (at == 0 || !matches!(s.t(at - 1), "=" | "!" | "<" | ">")))
        {
            return Some(at);
        }
        if matches!(s.t(at), "(" | "[" | "{") {
            at = s.close_of(at)?;
        }
        at += 1;
    }
    None
}

/// Required accessibility of the new field only. Existing type/field visibility is untouched.
/// A private field is visible throughout its declaring module and its descendants.
pub(super) fn visibility(
    declaration: (&std::path::Path, &str, usize),
    constructors: &[(std::path::PathBuf, String, usize)],
) -> &'static str {
    let (path, text, at) = declaration;
    let modules = modules_at(text, at);
    let mut needed = "";
    for (other, source, open) in constructors {
        if other == path && modules_at(source, *open).starts_with(&modules) {
            continue;
        }
        if !targets::same_crate(path, other) {
            return "pub ";
        }
        needed = "pub(crate) ";
    }
    needed
}

fn modules_at(text: &str, at: usize) -> Vec<String> {
    let Ok(s) = Source::new(text) else {
        return vec![];
    };
    (0..s.tokens.len())
        .filter_map(|i| {
            if !s.is(i, "mod") || !s.is(i + 2, "{") {
                return None;
            }
            let end = s.close_of(i + 2)?;
            (s.tokens[i + 2].start < at && at < s.tokens[end].start).then(|| s.t(i + 1).to_string())
        })
        .collect()
}
