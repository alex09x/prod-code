/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{SourceToken, TokenKind};

/// Tokenize source code into structural tokens with byte spans, ignoring comments and whitespace.
pub fn tokenize_source(source: &str) -> Vec<SourceToken> {
    let mut tokens = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];

        // Whitespace
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Single-line comment `//`
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Single-line comment `#` (Python, Shell, etc.)
        if b == b'#' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Multi-line comment `/* ... */`
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < bytes.len() {
                i += 2;
            } else {
                i = bytes.len();
            }
            continue;
        }

        // Delimiters
        if matches!(b, b'(' | b'[' | b'{') {
            tokens.push(SourceToken {
                kind: TokenKind::OpenDelim(b as char),
                start_byte: i,
                end_byte: i + 1,
            });
            i += 1;
            continue;
        }
        if matches!(b, b')' | b']' | b'}') {
            tokens.push(SourceToken {
                kind: TokenKind::CloseDelim(b as char),
                start_byte: i,
                end_byte: i + 1,
            });
            i += 1;
            continue;
        }

        // String literals (", ', `)
        if matches!(b, b'"' | b'\'' | b'`') {
            let quote = b;
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < bytes.len() {
                i += 1;
            }
            tokens.push(SourceToken {
                kind: TokenKind::StringLit(source[start..i].to_string()),
                start_byte: start,
                end_byte: i,
            });
            continue;
        }

        // Identifiers
        if b.is_ascii_alphabetic() || b == b'_' || b == b'$' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$')
            {
                i += 1;
            }
            tokens.push(SourceToken {
                kind: TokenKind::Ident(source[start..i].to_string()),
                start_byte: start,
                end_byte: i,
            });
            continue;
        }

        // Numbers
        if b.is_ascii_digit() {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_')
            {
                i += 1;
            }
            tokens.push(SourceToken {
                kind: TokenKind::NumberLit(source[start..i].to_string()),
                start_byte: start,
                end_byte: i,
            });
            continue;
        }

        // Multi-char punctuation
        let p_start = i;
        if i + 1 < bytes.len() && b.is_ascii() && bytes[i + 1].is_ascii() {
            let pair = &source[i..i + 2];
            if matches!(
                pair,
                "==" | "!="
                    | "<="
                    | ">="
                    | "&&"
                    | "||"
                    | "->"
                    | "::"
                    | "+="
                    | "-="
                    | "*="
                    | "/="
                    | ":="
                    | "=>"
                    | "??"
                    | "?."
                    | "<<"
                    | ">>"
                    | "**"
                    | "//"
            ) {
                tokens.push(SourceToken {
                    kind: TokenKind::Punct(pair.to_string()),
                    start_byte: p_start,
                    end_byte: p_start + 2,
                });
                i += 2;
                continue;
            }
        }

        // Non-ASCII Unicode character
        if !b.is_ascii() {
            let ch = source[i..].chars().next().unwrap();
            let ch_len = ch.len_utf8();
            tokens.push(SourceToken {
                kind: if ch.is_alphabetic() {
                    TokenKind::Ident(ch.to_string())
                } else {
                    TokenKind::Punct(ch.to_string())
                },
                start_byte: p_start,
                end_byte: p_start + ch_len,
            });
            i += ch_len;
            continue;
        }

        // Single-char punctuation (ASCII)
        tokens.push(SourceToken {
            kind: TokenKind::Punct((b as char).to_string()),
            start_byte: p_start,
            end_byte: p_start + 1,
        });
        i += 1;
    }

    tokens
}

/// Compute matching delimiter indices for fast balance checking.
pub fn compute_matching_delims(tokens: &[SourceToken]) -> Vec<Option<usize>> {
    let mut match_map = vec![None; tokens.len()];
    let mut stack: Vec<(char, usize)> = Vec::new();

    for (idx, tok) in tokens.iter().enumerate() {
        match tok.kind {
            TokenKind::OpenDelim(ch) => stack.push((ch, idx)),
            TokenKind::CloseDelim(ch) => {
                let expected_open = match ch {
                    ')' => '(',
                    ']' => '[',
                    '}' => '{',
                    _ => continue,
                };
                if let Some((open_ch, open_idx)) = stack.pop()
                    && open_ch == expected_open
                {
                    match_map[open_idx] = Some(idx);
                    match_map[idx] = Some(open_idx);
                }
            }
            _ => {}
        }
    }
    match_map
}
