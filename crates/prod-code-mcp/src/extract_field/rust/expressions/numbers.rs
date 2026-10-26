/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::fixture::builder::lexer::{Kind, Source};
use anyhow::Result;

/// Complete Rust token spans locally: the shared fixture lexer has simpler
/// numeric and identifier scanners. An exponent sign belongs to its literal;
/// Unicode XID continuations belong to their original (possibly raw) identifier.
pub(super) fn source(text: &str) -> Result<Source<'_>> {
    let mut s = Source::new(text)?;
    let mut tokens = Vec::with_capacity(s.tokens.len());
    let mut i = 0;
    while i < s.tokens.len() {
        let mut token = s.tokens[i];
        if token.kind == Kind::Literal && text.as_bytes()[token.start].is_ascii_digit() {
            token.end = end(text, token.start);
        } else if matches!(token.kind, Kind::Ident | Kind::Lifetime) {
            token.end = suffix(text, token.end);
        }
        i += 1;
        while i < s.tokens.len() && s.tokens[i].start < token.end {
            i += 1;
        }
        tokens.push(token);
    }
    s.tokens = tokens;
    Ok(s)
}

fn end(text: &str, start: usize) -> usize {
    let b = text.as_bytes();
    let mut i = start;
    if b.get(start) == Some(&b'0') && matches!(b.get(start + 1), Some(b'x' | b'o' | b'b')) {
        // Based integer digits and their suffix are already one shared-lexer token.
        while b
            .get(i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            i += 1;
        }
        return suffix(text, i);
    }
    while b.get(i).is_some_and(|c| c.is_ascii_digit() || *c == b'_') {
        i += 1;
    }
    if b.get(i) == Some(&b'.')
        && b.get(i + 1) != Some(&b'.')
        && !text[i + 1..]
            .chars()
            .next()
            .is_some_and(|c| c == '_' || unicode_ident::is_xid_start(c))
    {
        i += 1;
        while b.get(i).is_some_and(|c| c.is_ascii_digit() || *c == b'_') {
            i += 1;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        while b.get(i).is_some_and(|c| c.is_ascii_digit() || *c == b'_') {
            i += 1;
        }
    }
    suffix(text, i)
}

fn suffix(text: &str, mut i: usize) -> usize {
    for c in text[i..].chars() {
        if c != '_' && !unicode_ident::is_xid_continue(c) {
            break;
        }
        i += c.len_utf8();
    }
    i
}
