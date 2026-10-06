/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::extract_field::helpers::{is_ident, self_type};
use crate::extract_field::types::Braces;

/// Every `impl` block in `text`: the type it is for, where `impl` starts, and its braces.
pub fn impl_blocks(text: &str) -> Vec<(String, usize, usize, usize)> {
    let mut out = Vec::new();
    for (at, _) in text.match_indices("impl") {
        let before = text[..at].trim_end_matches([' ', '\t']);
        let starts_item = before.is_empty()
            || before.ends_with('\n')
            || before.ends_with("unsafe")
            || before.ends_with('}');
        let after = &text[at + "impl".len()..];
        if !starts_item || !(after.starts_with('<') || after.starts_with(char::is_whitespace)) {
            continue;
        }
        let Some(open) = text[at..].find(['{', ';']).map(|i| at + i) else {
            continue;
        };
        if text.as_bytes()[open] != b'{' {
            continue;
        }
        let Some(ty) = self_type(&text[at + "impl".len()..open]) else {
            continue;
        };
        if let Some(close) = crate::parameter_object::matching_bracket(text, open) {
            out.push((ty, at, open, close));
        }
    }
    out
}

/// The method whose body contains `offset`, inside the `impl` braces `open..close`: its name,
/// its parameter list, and its body's braces.
pub fn method_at(
    text: &str,
    open: usize,
    close: usize,
    offset: usize,
) -> Option<(String, String, usize, usize)> {
    let mut best = None;
    for (at, _) in text[open..close].match_indices("fn ") {
        let at = open + at;
        if text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let name_at = at + "fn ".len();
        let Some((name, params_start, params_end)) = crate::signature::param_span(text, name_at)
        else {
            continue;
        };
        let Some(body_open) = text[params_end..close]
            .find(['{', ';'])
            .map(|i| params_end + i)
        else {
            continue;
        };
        if text.as_bytes()[body_open] != b'{' {
            continue;
        }
        let Some(body_close) = crate::parameter_object::matching_bracket(text, body_open) else {
            continue;
        };
        if body_open < offset && offset < body_close {
            best = Some((
                name,
                text[params_start..params_end].to_string(),
                body_open,
                body_close,
            ));
        }
    }
    best
}

/// The braces of the struct whose name starts at `name_at`, or `None` for a tuple or unit
/// struct.
pub fn struct_braces(text: &str, name_at: usize) -> Option<(usize, usize)> {
    let open = text[name_at..].find(['{', ';', '(']).map(|i| name_at + i)?;
    if text.as_bytes()[open] != b'{' {
        return None;
    }
    Some((open, crate::parameter_object::matching_bracket(text, open)?))
}

pub fn braces_kind(text: &str, open: usize, close: usize) -> Braces {
    // A bare `..` before the closing brace is a rest pattern; a literal's update syntax always
    // names the value it copies from (`..base`). That settles `matches!(x, Store { a, .. })`,
    // where nothing around the braces says which it is.
    let inner = text[open + 1..close].trim_end().trim_end_matches(',');
    let rest = inner.trim_end().ends_with("..");
    if rest || followed_by_pattern_cue(text, close + 1, 0) {
        return Braces::Pattern { rest };
    }
    Braces::Literal
}

/// Whether what follows `from` marks the text before it as a pattern: `=>`, a single `=`, `|`,
/// a `:` type ascription, `in` (a `for` loop) or `if` (a match guard). Braces nested in a
/// tuple, a slice, a variant or another struct — `Some(Store { a }) =>` — are a pattern when
/// the brackets around them are, so a `)`, `]`, `}` or `,` sends the question outward.
fn followed_by_pattern_cue(text: &str, from: usize, depth: usize) -> bool {
    let after = text[from..].trim_start();
    let word = |w: &str| {
        after.starts_with(w)
            && !after[w.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
    };
    if after.starts_with("=>")
        || (after.starts_with('=') && !after.starts_with("=="))
        || (after.starts_with('|') && !after.starts_with("||"))
        || (after.starts_with(':') && !after.starts_with("::"))
        || word("in")
        || word("if")
    {
        return true;
    }
    if depth < 8 && after.starts_with([')', ']', '}', ',']) {
        return enclosing_close(text, text.len() - after.len())
            .is_some_and(|close| followed_by_pattern_cue(text, close + 1, depth + 1));
    }
    false
}

/// The bracket that closes the group `at` sits in, skipping any group opened after it.
fn enclosing_close(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = at;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => i = crate::parameter_object::matching_bracket(text, i)?,
            b')' | b']' | b'}' => return Some(i),
            b'"' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                i = j;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Whether the name at `at` begins a value built with braces rather than a type named in an
/// `impl` header, a return type or a bound. Returns the opening brace.
pub fn constructor_brace(text: &str, at: usize, name: &str) -> Option<usize> {
    if !text[at..].starts_with(name) || text[at + name.len()..].starts_with(is_ident) {
        return None;
    }
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let before = text[line_start..at].trim();
    if before.starts_with("impl")
        || before.contains(" impl ")
        || before.ends_with("->")
        || before.ends_with("for")
        || before.ends_with("struct")
        || before.ends_with("enum")
    {
        return None;
    }
    let mut i = at + name.len();
    let rest = &text[i..];
    if let Some(generics) = rest.strip_prefix("::<") {
        let mut depth = 1i32;
        let mut end = None;
        for (j, c) in generics.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(j + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        i += "::<".len() + end?;
    }
    let skipped = text[i..].len() - text[i..].trim_start().len();
    let open = i + skipped;
    (text.as_bytes().get(open) == Some(&b'{')).then_some(open)
}

/// The braces of every `Self { … }` between `open` and `close`, leaving out `-> Self {`, where
/// the brace is a function body.
pub fn self_literals(text: &str, open: usize, close: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for (at, _) in text[open..close].match_indices("Self") {
        let at = open + at;
        if text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if let Some(brace) = constructor_brace(text, at, "Self")
            && brace < close
        {
            out.push(brace);
        }
    }
    out
}
