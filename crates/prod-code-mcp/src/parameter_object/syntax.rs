/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::types::Language;

pub fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

pub fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|b| *b == b'\n')
        .map_or(bytes.len(), |n| from + n)
}

/// Walks `text` from `from`, handing `visit` the offset and byte of everything that is code.
/// Comments are skipped; a string literal is reported by its two quotes only, so a bracket or a
/// comma inside it is never seen. `visit` returns `false` to stop.
///
/// A quote is a string in every one of these languages, and so is a backtick outside Python —
/// unlike Rust, where `'a` is a lifetime. Python's comment is `#`, and its `//` is division.
pub fn walk_code(
    text: &str,
    from: usize,
    language: Language,
    mut visit: impl FnMut(usize, u8) -> bool,
) {
    let bytes = text.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        let c = bytes[i];
        let rest = &bytes[i..];
        if language == Language::Python && c == b'#' {
            i = line_end(bytes, i);
            continue;
        }
        if language != Language::Python && rest.starts_with(b"//") {
            i = line_end(bytes, i);
            continue;
        }
        if language != Language::Python && rest.starts_with(b"/*") {
            i = find_bytes(&bytes[i + 2..], b"*/").map_or(bytes.len(), |n| i + 2 + n + 2);
            continue;
        }
        if language == Language::Python && (rest.starts_with(b"\"\"\"") || rest.starts_with(b"'''"))
        {
            if !visit(i, c) {
                return;
            }
            let Some(n) = find_bytes(&bytes[i + 3..], &rest[..3]) else {
                return;
            };
            let last = i + 3 + n + 2;
            if !visit(last, c) {
                return;
            }
            i = last + 1;
            continue;
        }
        if c == b'"' || c == b'\'' || (c == b'`' && language != Language::Python) {
            if !visit(i, c) {
                return;
            }
            // A Go raw string has no escapes; every other literal here does.
            let escapes = !(c == b'`' && language == Language::Go);
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != c {
                j += if escapes && bytes[j] == b'\\' { 2 } else { 1 };
            }
            if j >= bytes.len() || !visit(j, c) {
                return;
            }
            i = j + 1;
            continue;
        }
        if !visit(i, c) {
            return;
        }
        i += 1;
    }
}

/// The offset of the bracket that closes the one at `open`, with strings and comments skipped
/// the way the language writes them.
pub fn close_in(text: &str, open: usize, language: Language) -> Option<usize> {
    if !matches!(text.as_bytes().get(open), Some(b'(' | b'[' | b'{')) {
        return None;
    }
    let mut depth = 0i32;
    let mut found = None;
    walk_code(text, open, language, |i, c| {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    found = Some(i);
                    return false;
                }
            }
            _ => {}
        }
        true
    });
    found
}

/// Brackets of every kind nest; string and character literals and comments are skipped, so a
/// `}` in a string or an apostrophe in a comment does not end the block early.
pub fn matching_bracket(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if !matches!(bytes.get(open), Some(b'(' | b'[' | b'{')) {
        return None;
    }
    let mut i = open;
    let mut depth = 0i32;
    let mut in_str: Option<u8> = None;
    let mut escaped = false;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(quote) = in_str {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == quote {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                i = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
                continue;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = text[i + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |n| i + 2 + n + 2);
                continue;
            }
            b'"' => in_str = Some(b'"'),
            // A lifetime is not the start of a character literal.
            b'\''
                if bytes
                    .get(i + 1)
                    .is_some_and(|n| *n != b'_' && !n.is_ascii_alphabetic())
                    || bytes.get(i + 2) == Some(&b'\'') =>
            {
                in_str = Some(b'\'')
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

pub fn char_literal(code: &str, at: usize) -> bool {
    let rest = &code[at + 1..];
    match rest.chars().next() {
        Some('\\') => true,
        Some(c) => rest[c.len_utf8()..].starts_with('\'') || !(c == '_' || c.is_alphabetic()),
        None => false,
    }
}

/// The token of `code` (blanked by [`rust_code`]) that ends at `*end`, whitespace skipped: a
/// name, a string, or one other character; `*end` moves to its start.
pub fn token_before<'a>(code: &'a [u8], end: &mut usize) -> Option<&'a [u8]> {
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    while *end > 0 && code[*end - 1].is_ascii_whitespace() {
        *end -= 1;
    }
    let stop = *end;
    match code[..stop].last()? {
        b if word(*b) => {
            while *end > 0 && word(code[*end - 1]) {
                *end -= 1;
            }
        }
        // The ABI of `extern "C"`, whose inside is blank.
        b'"' => *end = code[..stop - 1].iter().rposition(|b| *b == b'"')?,
        _ => *end -= 1,
    }
    Some(&code[*end..stop])
}

pub fn split_args(inner: &str) -> Vec<String> {
    let bytes = inner.as_bytes();
    let (mut depth, mut i, mut last) = (0i32, 0usize, 0usize);
    let mut in_str: Option<u8> = None;
    let mut escaped = false;
    let mut in_closure_params = false;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(quote) = in_str {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == quote {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => in_str = Some(b'"'),
            b'\''
                if bytes
                    .get(i + 1)
                    .is_some_and(|n| *n != b'_' && !n.is_ascii_alphabetic())
                    || bytes.get(i + 2) == Some(&b'\'') =>
            {
                in_str = Some(b'\'')
            }
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' | b'>' => depth -= 1,
            // `||` is either an empty closure list or a logical or; neither opens anything.
            b'|' if bytes.get(i + 1) != Some(&b'|') && (i == 0 || bytes[i - 1] != b'|') => {
                in_closure_params = !in_closure_params;
            }
            b'|' if bytes.get(i + 1) == Some(&b'|') => i += 1,
            b',' if depth == 0 && !in_closure_params => {
                out.push(inner[last..i].trim().to_string());
                last = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let tail = inner[last..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}
