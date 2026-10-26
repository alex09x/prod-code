/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::{Language, syntax::walk_code};

/// Exclusive end of one string/template type token. Interpolation is type
/// syntax here: nested templates, object types, quoted literals and comments
/// must not expose their braces or backticks as annotation/body boundaries.
pub(super) fn end(text: &str, start: usize) -> Option<usize> {
    quoted(text, start, 0, &mut |_, _| {})
}

/// Immediate interpolation interiors, excluding their `${` and `}` delimiters.
/// Binding analysis can recurse into these without reinterpreting literal data.
pub(super) fn interpolations(text: &str, start: usize) -> Option<Vec<(usize, usize)>> {
    let mut ranges = Vec::new();
    quoted(text, start, 0, &mut |from, to| ranges.push((from, to)))?;
    Some(ranges)
}

fn quoted(
    text: &str,
    start: usize,
    depth: usize,
    visit: &mut impl FnMut(usize, usize),
) -> Option<usize> {
    if depth >= 64 {
        return None;
    }
    let bytes = text.as_bytes();
    let delimiter = *bytes.get(start)?;
    let mut at = start + 1;
    while at < bytes.len() {
        if bytes[at] == b'\\' {
            at += 2;
        } else if bytes[at] == delimiter {
            return Some(at + 1);
        } else if delimiter == b'`' && bytes[at..].starts_with(b"${") {
            let from = at + 2;
            at = interpolation(text, from, depth + 1, visit)?;
            if depth == 0 {
                visit(from, at - 1);
            }
        } else {
            at += 1;
        }
    }
    None
}

fn interpolation(
    text: &str,
    mut from: usize,
    depth: usize,
    visit: &mut impl FnMut(usize, usize),
) -> Option<usize> {
    let mut braces = 1;
    let view = super::ts_mask::line_view(text);
    loop {
        let mut literal = None;
        let mut close = None;
        // Keep the existing comment rules rather than interpreting comment
        // text as braces or opening quotes. Stop before each literal so the
        // same nesting-aware scanner owns its complete extent.
        walk_code(&view, from, Language::TypeScript, |at, byte| {
            match byte {
                b'\'' | b'"' | b'`' => {
                    literal = Some(at);
                    return false;
                }
                b'{' => braces += 1,
                b'}' => {
                    braces -= 1;
                    if braces == 0 {
                        close = Some(at + 1);
                        return false;
                    }
                }
                _ => {}
            }
            true
        });
        if close.is_some() {
            return close;
        }
        from = quoted(text, literal?, depth, visit)?;
    }
}
