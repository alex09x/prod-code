/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::borrow::Cow;

use crate::safe_delete_go::lex::opaque_end;

/// Minimal records shared by local manifests and imports. Comments and literal bodies stay
/// opaque; string values use Go's interpreted/raw semantics before dependency matching.
pub(super) fn tokens(source: &str) -> Vec<(Cow<'_, str>, bool)> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            space if space.is_ascii_whitespace() && space != b'\n' => at += 1,
            b'"' | b'`' | b'\'' => {
                let Ok(Some(end)) = opaque_end(source, at) else {
                    return Vec::new();
                };
                if bytes[at] != b'\'' {
                    // Ordinary Go strings may contain arbitrary bytes. An undecodable
                    // value cannot be an import path, but must not erase other imports.
                    let value = string_value(&source[at..end]).unwrap_or_default();
                    tokens.push((value, true));
                }
                at = end;
            }
            b'/' if matches!(bytes.get(at + 1), Some(b'/' | b'*')) => {
                let Ok(Some(end)) = opaque_end(source, at) else {
                    return Vec::new();
                };
                // A block comment with a newline is a newline for Go's semicolon
                // insertion. Keep that boundary between import specifications.
                if source[at..end].contains('\n') {
                    tokens.push(("\n".into(), false));
                }
                at = end;
            }
            b'\n' | b';' | b'(' | b')' => {
                tokens.push((source[at..at + 1].into(), false));
                at += 1;
            }
            b'=' if bytes.get(at + 1) == Some(&b'>') => {
                tokens.push((source[at..at + 2].into(), false));
                at += 2;
            }
            _ => {
                let start = at;
                while at < bytes.len()
                    && !bytes[at].is_ascii_whitespace()
                    && !matches!(bytes[at], b'(' | b')' | b';' | b'"' | b'`' | b'\'')
                    && !(bytes[at] == b'/' && matches!(bytes.get(at + 1), Some(b'/' | b'*')))
                {
                    at += 1;
                }
                tokens.push((source[start..at].into(), false));
            }
        }
    }
    tokens
}

fn string_value(literal: &str) -> Option<Cow<'_, str>> {
    let value = &literal[1..literal.len() - 1];
    if literal.starts_with('`') {
        // Go discards carriage returns in raw strings, but never interprets backslashes.
        return Some(if value.contains('\r') {
            value.replace('\r', "").into()
        } else {
            value.into()
        });
    }
    if !value.contains('\\') {
        return Some(value.into());
    }
    let mut bytes = value.bytes();
    let mut decoded = Vec::with_capacity(value.len());
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            decoded.push(byte);
            continue;
        }
        let escape = bytes.next()?;
        match escape {
            b'a' => decoded.push(7),
            b'b' => decoded.push(8),
            b'f' => decoded.push(12),
            b'n' => decoded.push(b'\n'),
            b'r' => decoded.push(b'\r'),
            b't' => decoded.push(b'\t'),
            b'v' => decoded.push(11),
            b'\\' | b'"' => decoded.push(escape),
            b'0'..=b'7' => {
                let mut octal = u32::from(escape - b'0');
                for _ in 0..2 {
                    let digit = bytes.next()?;
                    if !(b'0'..=b'7').contains(&digit) {
                        return None;
                    }
                    octal = octal * 8 + u32::from(digit - b'0');
                }
                decoded.push(u8::try_from(octal).ok()?);
            }
            b'x' | b'u' | b'U' => {
                let width = match escape {
                    b'x' => 2,
                    b'u' => 4,
                    _ => 8,
                };
                let mut code = 0;
                for _ in 0..width {
                    code = code * 16 + char::from(bytes.next()?).to_digit(16)?;
                }
                if escape == b'x' {
                    decoded.push(code as u8);
                } else {
                    let mut utf8 = [0; 4];
                    decoded
                        .extend_from_slice(char::from_u32(code)?.encode_utf8(&mut utf8).as_bytes());
                }
            }
            _ => return None,
        }
    }
    String::from_utf8(decoded).ok().map(Cow::Owned)
}
