/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::is_ident;

/// The offset of the `{` that encloses `at`.
pub(crate) fn enclosing_open_brace(text: &str, at: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in text[..at].char_indices().rev() {
        match c {
            '}' => depth += 1,
            '{' if depth == 0 => return Some(i),
            '{' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The derives on the item whose header starts at `header`: the words inside every
/// `#[derive(…)]` directly above it.
pub(crate) fn derives_above(text: &str, header: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text[..header].lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with("#[") && !line.starts_with("///") && !line.starts_with("//") {
            break;
        }
        if let Some(rest) = line.strip_prefix("#[derive(") {
            out.extend(
                rest.trim_end_matches(")]")
                    .split(',')
                    .map(|w| w.trim().rsplit("::").next().unwrap_or("").to_string())
                    .filter(|w| !w.is_empty()),
            );
        }
    }
    out
}

/// The end of the expression that starts at `from`: the first `;`, or `,` / `)` / `}` / `]` that
/// closes nothing opened after `from`.
pub(crate) fn expression_end(text: &str, from: usize) -> usize {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut prev = '\0';
    for (i, c) in text[from..].char_indices() {
        if in_str {
            if c == '"' && prev != '\\' {
                in_str = false;
            }
            prev = c;
            continue;
        }
        match c {
            '"' => in_str = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return from + i,
            ')' | ']' | '}' => depth -= 1,
            ',' | ';' if depth == 0 => return from + i,
            _ => {}
        }
        prev = c;
    }
    text.len()
}

/// `!(value)`, or `value` without its `!` when it already had one around a simple operand.
pub(crate) fn negation_of(value: &str) -> String {
    let v = value.trim();
    if let Some(rest) = v.strip_prefix('!')
        && !rest.starts_with('=')
        && rest
            .chars()
            .all(|c| is_ident(c) || c == '.' || c == '(' || c == ')' || c == ':')
    {
        return rest.to_string();
    }
    match v {
        "true" => return "false".to_string(),
        "false" => return "true".to_string(),
        _ => {}
    }
    format!("!({v})")
}

/// Whether the offset `at` is inside a string literal on its line.
pub(crate) fn inside_string(text: &str, at: usize) -> bool {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let mut quotes = 0;
    let mut prev = '\0';
    for c in text[line_start..at].chars() {
        if c == '"' && prev != '\\' {
            quotes += 1;
        }
        prev = c;
    }
    quotes % 2 == 1
}

/// Whether the `{` at `open` opens a struct literal or pattern (`Flags {`, `Self {`,
/// `crate::m::Flags {`) rather than a block (`if flag {`, `else {`, `=> {`). A type is named in
/// CamelCase, a condition or a binding is not: text alone cannot tell `if flag { x }` from a
/// struct named `flag`, and Rust's naming convention can.
pub(crate) fn is_struct_brace(text: &str, open: usize) -> bool {
    let before = text[..open].trim_end();
    let segment: String = before
        .chars()
        .rev()
        .take_while(|c| is_ident(*c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    segment.chars().next().is_some_and(char::is_uppercase)
}

/// Whether the struct braces that start at `open` are a pattern rather than an expression: a
/// pattern is followed by `=` (a `let`), `=>` or `|` (a match arm) or `if` (a guard).
pub(crate) fn braces_are_pattern(text: &str, open: usize) -> bool {
    let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
        return false;
    };
    let after = text[close + 1..].trim_start();
    (after.starts_with('=') && !after.starts_with("=="))
        || after.starts_with('|') && !after.starts_with("||")
        || after.starts_with("if ")
        || after.starts_with("=>")
}
