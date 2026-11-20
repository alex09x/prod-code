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
use crate::parameter_object::Language;

/// The end of the Rust lifetime or label starting with the `'` at byte `at` of `text`: `'a`,
/// `'_`, `'outer`, `'α`, `'r#type`. `None` for a character literal (`'a'`, `'\n'`).
pub(crate) fn rust_lifetime_end(text: &str, at: usize) -> Option<usize> {
    let rest = text.get(at + 1..)?;
    let name = rest.strip_prefix("r#").unwrap_or(rest);
    let prefix = rest.len() - name.len();
    let mut chars = name.char_indices();
    let (_, first) = chars.next()?;
    if !(first == '_' || unicode_ident::is_xid_start(first)) {
        return None;
    }
    let len = chars
        .find(|(_, c)| !unicode_ident::is_xid_continue(*c))
        .map_or(name.len(), |(i, _)| i);
    let end = at + 1 + prefix + len;
    (text.as_bytes().get(end) != Some(&b'\'')).then_some(end)
}

/// The end of the Rust raw string literal starting at byte `at` of `text`, if one does.
fn rust_raw_string_end(text: &str, at: usize) -> Option<usize> {
    let rest = &text[at..];
    let after_prefix = ["br", "cr", "r"]
        .iter()
        .find_map(|prefix| rest.strip_prefix(prefix))?;
    let hashes = after_prefix.len() - after_prefix.trim_start_matches('#').len();
    let body = after_prefix[hashes..].strip_prefix('"')?;
    let close = format!("\"{}", "#".repeat(hashes));
    let end = body
        .find(&close)
        .map_or(text.len(), |i| text.len() - body.len() + i + close.len());
    Some(end)
}

/// Tokenize source code into structural tokens with byte spans, ignoring comments and whitespace.
pub fn tokenize_source(source: &str) -> Vec<SourceToken> {
    tokenize_source_for_lang(source, None)
}

/// Tokenize source code for an optional target language into structural tokens with byte spans.
pub fn tokenize_source_for_lang(source: &str, lang: Option<Language>) -> Vec<SourceToken> {
    let mut tokens = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;
    let is_python = matches!(lang, Some(Language::Python));
    let is_rust = matches!(lang, Some(Language::Rust));

    while i < bytes.len() {
        let b = bytes[i];

        // Whitespace
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // A Rust raw string (`r#"…"#`, `br"…"`, `cr#"…"#`) is one literal: its `#` is not a
        // comment, and a quote inside it opens nothing.
        if is_rust && let Some(end) = rust_raw_string_end(source, i) {
            tokens.push(SourceToken {
                kind: TokenKind::StringLit(source[i..end].to_string()),
                start_byte: i,
                end_byte: end,
            });
            i = end;
            continue;
        }

        // Single-line comment `//` (not in Python where `//` is floor division)
        if !is_python && b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
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

        // Multi-line comment `/* ... */` (not in Python)
        if !is_python && b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
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

        // A Rust lifetime or label (`'_`, `'a`, `'outer:`) is not a character literal: read as
        // one, `SessionOp<'_>` swallowed the code up to the next quote, and every call after it
        // was missed (#2052). `'a'` is still the character.
        if is_rust
            && b == b'\''
            && let Some(end) = rust_lifetime_end(source, i)
        {
            tokens.push(SourceToken {
                kind: TokenKind::Ident(source[i..end].to_string()),
                start_byte: i,
                end_byte: end,
            });
            i = end;
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
