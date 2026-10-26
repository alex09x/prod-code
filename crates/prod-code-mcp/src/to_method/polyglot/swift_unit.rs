/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::make_static::Language;

/// An unknown inherited result binding cannot establish unit type. Instead prove
/// the body already has separate statements: a top-level closed expression,
/// followed across a newline/semicolon by a new call. Swift implicit returns
/// require one expression. No return is needed for this statement body, even if
/// its result type is nonunit (explicit returns/throws remain untouched).
/// This deliberately does not accept arbitrary multiline expressions or infer
/// unit from a qualified module name. Nested closure returns prove nothing.
/// The receiver-rewrite mask exposes string interpolation code. A string at
/// outer depth is an expression, so its interpolations cannot prove separate
/// statements. Strings inside call arguments remain inside that call's stack.
pub(super) fn separate_calls(body: &str) -> bool {
    let Ok(proof) = super::super::swift_regex::proof_body(body) else {
        return false;
    };
    let body = proof.as_str();
    let syntax = super::super::binding::mask(body, Language::Swift);
    let tokens = super::super::binding::words(body, Language::Swift);
    let mut stack = Vec::new();
    let mut closed = None;
    let mut words = tokens.iter().peekable();
    let mut comment_until = 0;
    for (at, byte) in body.bytes().enumerate() {
        if !syntax[at] {
            if stack.is_empty() && at >= comment_until {
                comment_until = comment_end(body, at);
                if byte == b'"' {
                    return false;
                }
            }
            continue;
        }
        if let Some(&&(start, word)) = words.peek()
            && start == at
        {
            words.next();
            if stack.is_empty()
                && let Some(end) = closed
                && (end + 1..at).any(|i| syntax[i] && matches!(body.as_bytes()[i], b'\n' | b';'))
                && !matches!(word, "as" | "is")
                && body
                    .bytes()
                    .enumerate()
                    .skip(at + word.len())
                    .find(|(i, b)| syntax[*i] && !b.is_ascii_whitespace())
                    .is_some_and(|(_, b)| b == b'(')
            {
                return true;
            }
        }
        if byte.is_ascii_whitespace() || byte == b';' {
            continue;
        }
        closed = None;
        match byte {
            b'(' | b'[' | b'{' => stack.push(match byte {
                b'(' => b')',
                b'[' => b']',
                _ => b'}',
            }),
            b')' | b']' | b'}' => {
                if stack.pop() != Some(byte) {
                    return false;
                }
                if stack.is_empty() && byte == b')' {
                    closed = Some(at);
                }
            }
            _ => {}
        }
    }
    false
}

// Quotes in comments are not string boundaries. This is only called at masked
// outer bytes, before any outer string (which ends the proof) can be entered.
fn comment_end(body: &str, at: usize) -> usize {
    let bytes = body.as_bytes();
    if bytes[at..].starts_with(b"//") {
        return body[at..].find('\n').map_or(body.len(), |end| at + end);
    }
    if !bytes[at..].starts_with(b"/*") {
        return at;
    }
    let mut depth = 1;
    let mut end = at + 2;
    while end < bytes.len() && depth > 0 {
        if bytes[end..].starts_with(b"/*") {
            depth += 1;
            end += 2;
        } else if bytes[end..].starts_with(b"*/") {
            depth -= 1;
            end += 2;
        } else {
            end += 1;
        }
    }
    end
}
