/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::tokens::Token;

/// A composite literal's type may contain function signatures, but its element
/// initializers remain executable in the surrounding scope.
pub(super) fn composite_type_end(
    ts: &[Token<'_>],
    paired: &[Option<usize>],
    at: usize,
) -> Option<usize> {
    if !matches!(ts.get(at)?.text, "[" | "map") {
        return None;
    }
    // A bracket following a value is indexing, including `a[0] * func()...()`.
    if ts[at].text == "["
        && at > 0
        && (ts[at - 1].literal
            || matches!(ts[at - 1].text, ")" | "]" | "}")
            || identifier(ts[at - 1].text))
    {
        return None;
    }
    let end = type_end(ts, paired, at, 0)?;
    (ts.get(end)?.text == "{").then_some(end)
}

/// Consume exactly a signature, rather than searching forward for any brace.
pub(super) fn signature_end(
    ts: &[Token<'_>],
    paired: &[Option<usize>],
    at: usize,
    depth: usize,
) -> Option<usize> {
    if depth > 128 || ts.get(at + 1)?.text != "(" {
        return None;
    }
    let end = paired[at + 1]? + 1;
    // A parenthesized result list and a single result type are both optional.
    if ts.get(end).is_some_and(|t| t.text == "(") {
        Some(paired[end]? + 1)
    } else {
        Some(type_end(ts, paired, end, depth + 1).unwrap_or(end))
    }
}

fn type_end(ts: &[Token<'_>], paired: &[Option<usize>], at: usize, depth: usize) -> Option<usize> {
    if depth > 128 {
        return None;
    }
    let next = |at| type_end(ts, paired, at, depth + 1);
    match ts.get(at)?.text {
        "*" => next(at + 1),
        "[" => next(paired[at]? + 1),
        "map" if ts.get(at + 1)?.text == "[" => next(paired[at + 1]? + 1),
        "chan" => next(at + 1 + usize::from(ts.get(at + 1)?.text == "<-")),
        "<-" if ts.get(at + 1)?.text == "chan" => next(at + 2),
        "func" => signature_end(ts, paired, at, depth + 1),
        "struct" | "interface" if ts.get(at + 1)?.text == "{" => Some(paired[at + 1]? + 1),
        "(" => Some(paired[at]? + 1),
        name if identifier(name) => {
            let mut end = at + 1;
            if ts.get(end).is_some_and(|t| t.text == ".") {
                if !identifier(ts.get(end + 1)?.text) {
                    return None;
                }
                end += 2;
            }
            if ts.get(end).is_some_and(|t| t.text == "[") {
                end = paired[end]? + 1;
            }
            Some(end)
        }
        _ => None,
    }
}

fn identifier(text: &str) -> bool {
    text.chars().next().is_some_and(super::tokens::ident)
        && !matches!(
            text,
            "return" | "case" | "range" | "var" | "const" | "type" | "if" | "for"
        )
}
