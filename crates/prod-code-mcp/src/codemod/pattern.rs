/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;

use super::types::{PatternToken, ReplacementToken, TokenKind};

/// Tokenize a pattern string with support for metavariables (`$var`).
pub fn tokenize_pattern(pattern: &str) -> Result<Vec<PatternToken>> {
    let mut tokens = Vec::new();
    let bytes = pattern.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Metavariable: `$name` or `$$$args`
        if b == b'$' {
            let start = i;
            i += 1;
            while i < bytes.len()
                && (bytes[i] == b'$' || bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
            {
                i += 1;
            }
            let name = &pattern[start + 1..i];
            if name.is_empty() {
                tokens.push(PatternToken::Literal(TokenKind::Punct("$".to_string())));
            } else {
                tokens.push(PatternToken::Metavar(name.to_string()));
            }
            continue;
        }

        // Delimiters
        if matches!(b, b'(' | b'[' | b'{') {
            tokens.push(PatternToken::Literal(TokenKind::OpenDelim(b as char)));
            i += 1;
            continue;
        }
        if matches!(b, b')' | b']' | b'}') {
            tokens.push(PatternToken::Literal(TokenKind::CloseDelim(b as char)));
            i += 1;
            continue;
        }

        // String literals
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
                i += 1; // closing quote
            }
            tokens.push(PatternToken::Literal(TokenKind::StringLit(
                pattern[start..i].to_string(),
            )));
            continue;
        }

        // Identifiers
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push(PatternToken::Literal(TokenKind::Ident(
                pattern[start..i].to_string(),
            )));
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
            tokens.push(PatternToken::Literal(TokenKind::NumberLit(
                pattern[start..i].to_string(),
            )));
            continue;
        }

        // Multi-char punctuation
        if i + 1 < bytes.len() && b.is_ascii() && bytes[i + 1].is_ascii() {
            let pair = &pattern[i..i + 2];
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
                tokens.push(PatternToken::Literal(TokenKind::Punct(pair.to_string())));
                i += 2;
                continue;
            }
        }

        // Non-ASCII Unicode character
        if !b.is_ascii() {
            let ch = pattern[i..].chars().next().unwrap();
            let ch_len = ch.len_utf8();
            tokens.push(PatternToken::Literal(if ch.is_alphabetic() {
                TokenKind::Ident(ch.to_string())
            } else {
                TokenKind::Punct(ch.to_string())
            }));
            i += ch_len;
            continue;
        }

        tokens.push(PatternToken::Literal(TokenKind::Punct(
            (b as char).to_string(),
        )));
        i += 1;
    }

    Ok(tokens)
}

/// Parse a replacement string into literal chunks and `$var` placeholders.
pub fn parse_replacement(rep: &str) -> Vec<ReplacementToken> {
    let mut tokens = Vec::new();
    let bytes = rep.as_bytes();
    let mut i = 0;
    let mut text_start = 0;

    while i < bytes.len() {
        if bytes[i] == b'$' {
            if i > text_start {
                tokens.push(ReplacementToken::Text(rep[text_start..i].to_string()));
            }
            let var_start = i + 1;
            i += 1;
            while i < bytes.len()
                && (bytes[i] == b'$' || bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
            {
                i += 1;
            }
            let name = &rep[var_start..i];
            if name.is_empty() {
                tokens.push(ReplacementToken::Text("$".to_string()));
            } else {
                tokens.push(ReplacementToken::Metavar(name.to_string()));
            }
            text_start = i;
        } else {
            i += 1;
        }
    }
    if text_start < bytes.len() {
        tokens.push(ReplacementToken::Text(rep[text_start..].to_string()));
    }
    tokens
}
