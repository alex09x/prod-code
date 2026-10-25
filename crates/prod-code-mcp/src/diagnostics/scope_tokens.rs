/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Context identity is a sequence of spellings, separate from the delimiter mask.
//! Never join words, punctuation or literal suffixes across actual trivia.
use crate::extract_trait::lex::{char_literal_end, raw_string_end};
use crate::extract_trait::types::is_ident;

pub(super) fn rust_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\t'..='\r' | ' ' | '\u{0085}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}'
    )
}

fn comment_end(text: &str, start: usize) -> usize {
    if text[start..].starts_with("//") {
        return text[start..].find('\n').map_or(text.len(), |n| start + n);
    }
    let bytes = text.as_bytes();
    let mut depth = 1;
    let mut i = start + 2;
    while i < bytes.len() && depth > 0 {
        match (bytes[i], bytes.get(i + 1)) {
            (b'/', Some(b'*')) => {
                depth += 1;
                i += 2;
            }
            (b'*', Some(b'/')) => {
                depth -= 1;
                i += 2;
            }
            _ => i += 1,
        }
    }
    i
}

/// Called only at the start of a non-comment span masked by lexical_code.
fn literal_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    if let Some(end) = raw_string_end(bytes, start) {
        return end;
    }
    let quote = if matches!(bytes[start], b'b' | b'c') {
        start + 1
    } else {
        start
    };
    if bytes[quote] == b'\'' {
        return char_literal_end(text, quote).unwrap_or(text.len());
    }
    let mut i = quote + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i = (i + 2).min(bytes.len()),
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

fn word_end(text: &str, start: usize) -> usize {
    start
        + text[start..]
            .find(|ch: char| !is_ident(ch))
            .unwrap_or(text.len() - start)
}

fn code_token_end(text: &str, start: usize) -> usize {
    let tail = &text[start..];
    let ch = tail.chars().next().unwrap();
    // Raw identifiers and lifetimes have meaningful adjacency to their prefix.
    if tail.starts_with("r#") && tail[2..].chars().next().is_some_and(is_ident) {
        return word_end(text, start + 2);
    }
    if ch == '\'' && tail[1..].chars().next().is_some_and(is_ident) {
        return word_end(text, start + 1);
    }
    if is_ident(ch) {
        let mut end = word_end(text, start);
        if ch.is_ascii_digit()
            && !tail.starts_with("0x")
            && !tail.starts_with("0o")
            && !tail.starts_with("0b")
        {
            // A decimal point and an exponent sign are part of a numeric token.
            // Ranges and field access instead begin a separate punctuation token.
            if text[end..].starts_with('.')
                && !text[end..].starts_with("..")
                && !text[end + 1..]
                    .chars()
                    .next()
                    .is_some_and(|c| c == '_' || unicode_ident::is_xid_start(c))
            {
                end = word_end(text, end + 1);
            }
            if matches!(text.as_bytes().get(end - 1), Some(b'e' | b'E'))
                && matches!(text.as_bytes().get(end), Some(b'+' | b'-'))
            {
                end = word_end(text, end + 1);
            }
        }
        return end;
    }
    for punct in [
        "<<=", ">>=", "..=", "...", "::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "<<",
        ">>", "+=", "-=", "*=", "/=", "%=", "^=", "&=", "|=", "..",
    ] {
        if tail.starts_with(punct) {
            return start + punct.len();
        }
    }
    start + ch.len_utf8()
}

pub(super) fn normalized_code(text: &str, mask: &[bool], start: usize, end: usize) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut i = start;
    while i < end {
        if !mask[i] && (text[i..].starts_with("//") || text[i..].starts_with("/*")) {
            i = comment_end(text, i);
            continue;
        }
        if mask[i] && rust_whitespace(text[i..].chars().next().unwrap()) {
            i += text[i..].chars().next().unwrap().len_utf8();
            continue;
        }
        let next = if !mask[i] {
            // Keep the complete spelling, including interior whitespace/comments/braces.
            // A suffix is part of the literal only when directly adjacent.
            word_end(text, literal_end(text, i))
        } else {
            code_token_end(text, i)
        };
        tokens.push(text[i..next.min(end)].to_owned());
        i = next;
    }
    tokens
}
