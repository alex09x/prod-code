/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::call_sites::opaque_at;

/// Conservative Rust whitespace equivalence. Token order, punctuation, comments and literal
/// bytes are retained. Import sorting, optional commas and closure rewrites are deliberately
/// uncertain here: without a full syntax proof they must still receive platform warnings.
/// Callers also need Rust file and formatter-command evidence before suppressing a warning.
pub fn layout_only(old: &[u8], new: &[u8]) -> bool {
    if old == new {
        return false;
    }
    let (Ok(old), Ok(new)) = (std::str::from_utf8(old), std::str::from_utf8(new)) else {
        return false;
    };
    matches!((tokens(old), tokens(new)), (Some(a), Some(b)) if a == b)
}

fn tokens(text: &str) -> Option<Vec<&str>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let start = i;
        let ch = text[i..].chars().next()?;
        if ch.is_whitespace() {
            i += ch.len_utf8();
            continue;
        }
        let mut opaque = opaque_at(text, i, 0).ok()?;
        if opaque.is_none()
            && matches!(ch, 'b' | 'c')
            && text
                .as_bytes()
                .get(i + 1)
                .is_some_and(|b| matches!(b, b'r' | b'"' | b'\''))
        {
            opaque = opaque_at(text, i + 1, 0).ok()?;
        }
        if let Some((end, _)) = opaque {
            // Prefix separation changes b"x" into b "x", and a lifetime can be split too.
            // Decline all these edge forms instead of treating their gaps as whitespace.
            if matches!(ch, '"' | '\'')
                && i > 0
                && text[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                return None;
            }
            if text[end..].starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                return None;
            }
            i = end;
        } else if ch.is_ascii_digit() {
            i += 1;
            // Keep numeric spelling and adjoining operators intact too; floats, exponents,
            // suffixes and macro literal inputs cannot be proven by splitting on punctuation.
            while text[i..].starts_with(|c: char| !c.is_whitespace() && !"()[]{};,\"'".contains(c))
            {
                i += text[i..].chars().next()?.len_utf8();
            }
        } else if unicode_ident::is_xid_continue(ch) || ch == '\'' {
            i += ch.len_utf8();
            while let Some(c) = text[i..].chars().next() {
                if !unicode_ident::is_xid_continue(c) {
                    break;
                }
                i += c.len_utf8();
            }
        } else if "()[]{};,".contains(ch) {
            i += ch.len_utf8();
        } else {
            // Preserve operator jointness: `>>` and `> >`, or `&&` and `& &`, differ.
            i += ch.len_utf8();
            while let Some(c) = text[i..].chars().next() {
                if c.is_whitespace()
                    || unicode_ident::is_xid_continue(c)
                    || "\"'()[]{};,".contains(c)
                    || text[i..].starts_with("//")
                    || text[i..].starts_with("/*")
                {
                    break;
                }
                i += c.len_utf8();
            }
        }
        out.push(&text[start..i]);
    }
    Some(out)
}

pub(super) fn rust_formatter(command: &[String]) -> bool {
    matches!(command, [cargo, fmt, ..] if cargo == "cargo" && fmt == "fmt")
        || matches!(command, [formatter, ..] if formatter == "rustfmt")
}
