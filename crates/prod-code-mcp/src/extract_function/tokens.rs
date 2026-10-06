/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn mentions(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        let before_ok = at == 0 || !is_ident(text[..at].chars().last().unwrap());
        let after = at + name.len();
        let after_ok = after == text.len() || !is_ident(text[after..].chars().next().unwrap());
        before_ok && after_ok
    })
}

/// What a token of Rust source is, as far as matching copies needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Word,
    Number,
    Str,
    Char,
    Punct,
}

/// The tokens of `text`, as (kind, start, end): names and keywords, number, string and char
/// literals, and every other character on its own. Whitespace and `//` comments are skipped.
pub fn tokens(text: &str) -> Vec<(Token, usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = text[i..].chars().next().unwrap_or(' ');
        let len = c.len_utf8();
        if c.is_whitespace() {
            i += len;
        } else if text[i..].starts_with("//") {
            i = text[i..].find('\n').map_or(text.len(), |n| i + n);
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'.')
            {
                // `1..3` is two numbers and a range, not one number.
                if bytes[j] == b'.' && bytes.get(j + 1) == Some(&b'.') {
                    break;
                }
                j += 1;
            }
            out.push((Token::Number, i, j));
            i = j;
        } else if c == '"' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'"' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let j = (j + 1).min(bytes.len());
            out.push((Token::Str, i, j));
            i = j;
        } else if c == '\'' {
            // A char literal closes within a few bytes; a lifetime does not close at all.
            let close = text[i + 1..]
                .char_indices()
                .take(4)
                .find(|(k, ch)| *ch == '\'' && *k > 0)
                .map(|(k, _)| i + 1 + k);
            match close {
                Some(end)
                    if !text[i + 1..end].starts_with(|ch: char| ch.is_alphabetic())
                        || end - i <= 3 =>
                {
                    out.push((Token::Char, i, end + 1));
                    i = end + 1;
                }
                _ => {
                    let mut j = i + 1;
                    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
                    {
                        j += 1;
                    }
                    out.push((Token::Word, i, j));
                    i = j;
                }
            }
        } else if is_ident(c) {
            let mut j = i;
            while let Some(ch) = text[j..].chars().next() {
                if !is_ident(ch) {
                    break;
                }
                j += ch.len_utf8();
            }
            out.push((Token::Word, i, j));
            i = j;
        } else {
            out.push((Token::Punct, i, i + len));
            i += len;
        }
    }
    out
}
