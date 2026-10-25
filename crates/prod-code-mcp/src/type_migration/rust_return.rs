/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Rust function return boundaries, independent of the other languages' signatures.

use crate::extract_trait::lex::{next_code, previous_code};

/// Skip a balanced generic header, parameter list or type/macro delimiter. Ordinary
/// token trees own their angles, so comparisons inside const blocks cannot close a header.
fn after_group(text: &str, code: &[bool], open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let close = match bytes[open] {
        b'<' => b'>',
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };
    let mut i = open + 1;
    while i < bytes.len() {
        if !code[i] {
            i += 1;
            continue;
        }
        if bytes[i] == close && !(close == b'>' && bytes.get(i - 1) == Some(&b'-')) {
            return Some(i + 1);
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => i = after_group(text, code, i)?,
            b'<' if close == b'>' => i = after_group(text, code, i)?,
            b')' | b']' | b'}' => return None,
            b';' if close == b'>' => return None,
            _ => i += 1,
        }
    }
    None
}

fn where_at(text: &str, at: usize) -> bool {
    let ident = |c: char| unicode_ident::is_xid_continue(c) || c == '_';
    text[at..].starts_with("where")
        && !text[..at].ends_with(ident)
        && !text[..at].ends_with("r#")
        && !text[at + 5..].starts_with(ident)
}

/// Only the immediate arrow after this function's parameters introduces its return.
/// At top level, `where`, a trait semicolon and the body end the return type. A macro
/// delimiter belongs to the type, as do groups within generic/array/function types.
pub(super) fn return_span(text: &str, code: &[bool], mut at: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    if bytes[at] == b'<' {
        at = next_code(text, code, after_group(text, code, at)?)?;
    }
    if bytes[at] != b'(' {
        return None;
    }
    at = next_code(text, code, after_group(text, code, at)?)?;
    if !text[at..].starts_with("->") {
        return None;
    }
    let start = next_code(text, code, at + 2)?;
    let mut end = start;
    at = start;
    while at < bytes.len() {
        if !code[at] || bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        let macro_brace = bytes[at] == b'{'
            && previous_code(text, code, at).is_some_and(|i| i > start && bytes[i] == b'!');
        if where_at(text, at) || bytes[at] == b';' || (bytes[at] == b'{' && !macro_brace) {
            return (end > start).then_some((start, end));
        }
        match bytes[at] {
            b'(' | b'[' | b'{' | b'<' => at = after_group(text, code, at)?,
            b'}' => return None,
            _ => at += text[at..].chars().next()?.len_utf8(),
        }
        end = at;
    }
    None
}
