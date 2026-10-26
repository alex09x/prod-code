/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};

/// C++ concatenates raw and ordinary string tokens, including encoding prefixes.
/// Keep each token whole so expression parsing can join the complete sequence.
pub(super) fn cpp_string_end(rest: &str) -> Result<Option<usize>> {
    if let Some(end) = cpp_raw_end(rest)? {
        return Ok(Some(end));
    }
    let Some(prefix) = ["u8\"", "u\"", "U\"", "L\"", "\""]
        .into_iter()
        .find(|prefix| rest.starts_with(prefix))
    else {
        return Ok(None);
    };
    let mut end = prefix.len();
    while let Some(c) = rest[end..].chars().next() {
        end += c.len_utf8();
        if c == '"' {
            return Ok(Some(cpp_suffix_end(rest, end)?));
        }
        if c == '\\' {
            end += rest[end..]
                .chars()
                .next()
                .context("unclosed C++ string")?
                .len_utf8();
        }
    }
    anyhow::bail!("unclosed C++ string")
}

pub(super) fn cpp_string(literal: &str) -> bool {
    [
        "\"", "u8\"", "u\"", "U\"", "L\"", "R\"", "u8R\"", "uR\"", "UR\"", "LR\"",
    ]
    .into_iter()
    .any(|prefix| literal.starts_with(prefix))
}

/// Raw contents may contain ordinary quotes. Only the exact custom delimiter closes them.
pub(super) fn cpp_raw_end(rest: &str) -> Result<Option<usize>> {
    let Some(prefix) = ["R\"", "u8R\"", "uR\"", "UR\"", "LR\""]
        .into_iter()
        .find(|prefix| rest.starts_with(prefix))
    else {
        return Ok(None);
    };
    let open = rest
        .find('(')
        .context("unclosed C++ raw string delimiter")?;
    let delimiter = &rest[prefix.len()..open];
    anyhow::ensure!(
        delimiter.len() <= 16
            && delimiter
                .bytes()
                .all(|b| b.is_ascii_graphic() && !matches!(b, b'(' | b')' | b'\\')),
        "unsupported C++ raw string delimiter"
    );
    let close = format!("){delimiter}\"");
    let end = rest[open + 1..]
        .find(&close)
        .context("unclosed C++ raw string")?;
    Ok(Some(cpp_suffix_end(rest, open + 1 + end + close.len())?))
}

fn cpp_suffix_end(rest: &str, mut end: usize) -> Result<usize> {
    // A user-defined suffix belongs to the literal token. Leaving it separate
    // would allow a partial selection to turn `R"(v)"_n` into `limit_n`.
    let start = end;
    while let Some(c) = rest[end..].chars().next() {
        if super::tokens::ident(c) || (end > start && c.is_ascii_digit()) {
            end += c.len_utf8();
        } else if c == '\\' {
            // Universal character names are also permitted in suffix identifiers.
            let width = match rest.as_bytes().get(end + 1) {
                Some(b'u') => 4,
                Some(b'U') => 8,
                _ => anyhow::bail!("unsupported C++ literal suffix escape"),
            };
            let digits = rest
                .get(end + 2..end + 2 + width)
                .context("incomplete C++ literal suffix escape")?;
            anyhow::ensure!(
                digits.bytes().all(|b| b.is_ascii_hexdigit()),
                "unsupported C++ literal suffix escape"
            );
            end += 2 + width;
        } else {
            break;
        }
    }
    Ok(end)
}

/// Keep prefixed Python strings in one token, including triple quoted strings.
pub(super) fn python_prefixed_end(rest: &str) -> Result<Option<usize>> {
    let Some(prefix) = ["br", "rb", "fr", "rf", "r", "b", "u", "f"]
        .into_iter()
        .find(|p| {
            rest.get(..p.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(p))
                && rest
                    .as_bytes()
                    .get(p.len())
                    .is_some_and(|c| matches!(c, b'\'' | b'"'))
        })
    else {
        return Ok(None);
    };
    let quoted = &rest[prefix.len()..];
    let width = if quoted.starts_with("\"\"\"") || quoted.starts_with("'''") {
        3
    } else {
        1
    };
    let quote = &quoted[..width];
    let mut at = prefix.len() + width;
    while at < rest.len() && !rest[at..].starts_with(quote) {
        if rest.as_bytes()[at] == b'\\' {
            at += 1;
        }
        at += rest
            .get(at..)
            .and_then(|s| s.chars().next())
            .context("unclosed prefixed string")?
            .len_utf8();
    }
    anyhow::ensure!(at < rest.len(), "unclosed prefixed string");
    Ok(Some(at + width))
}

/// We do not parse interpolation grammar. Escaped braces and static f-string text are data.
pub(super) fn python_interpolated(literal: &str) -> bool {
    let prefix = literal.split(['\'', '"']).next().unwrap_or("");
    if !prefix.bytes().any(|b| matches!(b, b'f' | b'F')) {
        return false;
    }
    let mut chars = literal[prefix.len()..].chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if chars.peek() == Some(&'{') {
                chars.next();
            } else {
                return true;
            }
        }
    }
    false
}
