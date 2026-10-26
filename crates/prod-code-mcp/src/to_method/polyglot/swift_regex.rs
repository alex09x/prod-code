/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::caller_migration::lexical_code_mask;
use crate::parameter_object::Language;
use anyhow::{Result, ensure};

/// A local view for the two Swift return proofs, never a rewritten method body.
/// Treat each extended regex as one opaque expression. Its pattern's keywords,
/// delimiters and separators are literal text, including in call arguments.
/// Keep offsets and line breaks so multiline implicit values still fail closed.
pub(super) fn proof_body(body: &str) -> Result<String> {
    let (proof, _, complete) = opaque(body);
    ensure!(
        complete,
        "unsupported unterminated Swift regex return proof"
    );
    Ok(proof)
}

/// The same executable view feeds receiver uses, callers and declaration/body
/// boundaries. The shared Swift string scanner still exposes actual string
/// interpolation expressions. Swift regex patterns (including regex escapes
/// that resemble interpolation) are opaque; they are never receiver code.
pub(super) fn executable_mask(text: &str) -> Vec<bool> {
    let (view, ranges, _) = opaque(text);
    let mut syntax = lexical_code_mask(&view, Language::Swift);
    for range in ranges {
        syntax[range].fill(false);
    }
    syntax
}

/// Split against an opaque regex view, then recover original byte spans. Keep
/// both ends of each placeholder nonblank so trimming never drops pattern bytes.
pub(super) fn arguments(text: &str) -> Option<Vec<String>> {
    let (view, ranges, complete) = opaque(text);
    if !complete {
        return None;
    }
    let mut bytes = view.into_bytes();
    for range in ranges {
        bytes[range.end - 1] = b'0';
    }
    let view = String::from_utf8(bytes).ok()?;
    let mut offset = 0;
    crate::to_method::helpers::split_call_arguments(&view)
        .into_iter()
        .map(|arg| {
            let start = offset + view[offset..].find(&arg)?;
            offset = start + arg.len();
            Some(text[start..offset].to_string())
        })
        .collect()
}

fn opaque(text: &str) -> (String, Vec<std::ops::Range<usize>>, bool) {
    let mut proof = text.to_owned();
    let mut syntax = lexical_code_mask(&proof, Language::Swift);
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut complete = true;
    let mut at = 0;
    while at < bytes.len() {
        if !syntax[at] {
            at += 1;
            continue;
        }
        let hashes = bytes[at..].iter().take_while(|&&b| b == b'#').count();
        if bytes.get(at + hashes) != Some(&b'/') || (hashes == 0 && !bare_start(bytes, &syntax, at))
        {
            at += hashes.max(1);
            continue;
        }
        let end = match literal_end(bytes, at + hashes + 1, hashes) {
            Some(end) => end,
            // Bare slash syntax is ambiguous with prefix operators, including
            // `/+`. Without a closing delimiter on this line it remains code.
            // Extended delimiters are unambiguous and still fail closed.
            None if hashes == 0 => {
                at += 1;
                continue;
            }
            None => {
                complete = false;
                bytes.len()
            }
        };
        let mut opaque: Vec<u8> = bytes[at..end]
            .iter()
            .map(|&b| if matches!(b, b'\n' | b'\r') { b } else { b' ' })
            .collect();
        opaque[0] = b'0';
        proof.replace_range(
            at..end,
            std::str::from_utf8(&opaque).expect("ASCII regex view"),
        );
        // Quotes and comment markers inside the pattern may have confused the
        // receiver mask. Recompute after hiding the entire literal, so actual
        // syntax following it (including another regex argument) stays visible.
        syntax = lexical_code_mask(&proof, Language::Swift);
        ranges.push(at..end);
        at = end;
    }
    (proof, ranges, complete)
}

fn bare_start(bytes: &[u8], syntax: &[bool], at: usize) -> bool {
    // Swift operators are maximal tokens. A slash adjacent to an operator head
    // belongs to that token (for example +/), not to a following regex literal.
    if at > 0
        && syntax[at - 1]
        && matches!(
            bytes[at - 1],
            b'/' | b'='
                | b'-'
                | b'+'
                | b'!'
                | b'*'
                | b'%'
                | b'<'
                | b'>'
                | b'&'
                | b'|'
                | b'^'
                | b'?'
                | b'~'
                | b'.'
        )
    {
        return false;
    }
    if bytes
        .get(at + 1)
        .is_none_or(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'*' | b'='))
    {
        return false;
    }
    let previous = (0..at)
        .rev()
        .find(|&i| syntax[i] && !bytes[i].is_ascii_whitespace());
    let Some(previous) = previous else {
        return comment_gap(&bytes[..at]);
    };
    // A masked string is still a value to the left of division. Only comments
    // and whitespace may separate an expression prefix from a regex literal.
    if !comment_gap(&bytes[previous + 1..at]) {
        return false;
    }
    if matches!(
        bytes[previous],
        b'=' | b'('
            | b'['
            | b'{'
            | b','
            | b':'
            | b';'
            | b'?'
            | b'!'
            | b'&'
            | b'|'
            | b'+'
            | b'-'
            | b'*'
            | b'%'
            | b'^'
            | b'~'
    ) {
        return true;
    }
    let start = bytes[..=previous]
        .iter()
        .rposition(|b| !b.is_ascii_alphanumeric() && *b != b'_')
        .map_or(0, |i| i + 1);
    matches!(
        &bytes[start..=previous],
        b"return"
            | b"throw"
            | b"case"
            | b"in"
            | b"if"
            | b"while"
            | b"guard"
            | b"switch"
            | b"where"
            | b"try"
    ) && bytes[..start]
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .is_none_or(|i| bytes[i] != b'.')
}

fn comment_gap(mut gap: &[u8]) -> bool {
    while !gap.is_empty() {
        gap = gap.trim_ascii_start();
        if gap.starts_with(b"//") {
            gap = gap
                .iter()
                .position(|b| *b == b'\n')
                .map_or(&[], |i| &gap[i..]);
        } else if gap.starts_with(b"/*") {
            let mut depth = 1;
            let mut at = 2;
            while at < gap.len() && depth > 0 {
                if gap[at..].starts_with(b"/*") {
                    depth += 1;
                    at += 2;
                } else if gap[at..].starts_with(b"*/") {
                    depth -= 1;
                    at += 2;
                } else {
                    at += 1;
                }
            }
            gap = &gap[at..];
        } else {
            return gap.is_empty();
        }
    }
    true
}

fn literal_end(bytes: &[u8], mut at: usize, hashes: usize) -> Option<usize> {
    let multiline = bytes.get(at) == Some(&b'\n') || bytes[at..].starts_with(b"\r\n");
    let mut in_class = false;
    let mut parens = 0usize;
    while at < bytes.len() {
        if bytes[at] == b'\\' {
            if hashes == 0 && bytes.get(at + 1).is_none_or(|b| matches!(b, b'\n' | b'\r')) {
                return None;
            }
            at += 2;
        } else if hashes == 0 && matches!(bytes[at], b'\n' | b'\r') {
            return None;
        } else if bytes[at] == b'[' {
            in_class = true;
            at += 1;
        } else if bytes[at] == b']' {
            in_class = false;
            at += 1;
        } else if hashes == 0 && !in_class && bytes[at] == b'(' {
            parens += 1;
            at += 1;
        } else if hashes == 0 && !in_class && bytes[at] == b')' {
            // SE-0354: bare regexes cannot contain an unbalanced closing ')'.
            if parens == 0 {
                return None;
            }
            parens -= 1;
            at += 1;
        } else if hashes == 0
            && !in_class
            && bytes[at] == b'/'
            && bytes[at - 1].is_ascii_whitespace()
            && bytes[..at - 1]
                .iter()
                .rev()
                .take_while(|&&b| b == b'\\')
                .count()
                % 2
                == 0
        {
            // Bare regexes cannot end in unescaped whitespace. A later
            // division slash or comment cannot close `/+value / 2`.
            return None;
        } else if bytes[at] == b'/'
            && !in_class
            && bytes
                .get(at + 1..at + 1 + hashes)
                .is_some_and(|suffix| suffix.iter().all(|&b| b == b'#'))
            && (!multiline
                || bytes[..at]
                    .iter()
                    .rposition(|&b| b == b'\n')
                    .is_some_and(|line| bytes[line + 1..at].iter().all(u8::is_ascii_whitespace)))
        {
            return Some(at + 1 + hashes);
        } else {
            at += 1;
        }
    }
    None
}
