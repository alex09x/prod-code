/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::case::is_ident;

/// Find the outer expression boundary, protecting delimiters in qualified paths,
/// turbofish arguments, closure parameters and closure return types.
pub(super) fn expression_end(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    let mask =
        crate::caller_migration::lexical_code_mask(text, crate::parameter_object::Language::Rust);
    let mut angles = 0usize;
    let mut closure_params = false;
    let mut closure_ret = false;
    let mut expect_operand = true;
    let mut i = from;
    let has_arrow = |at: usize| {
        text[at..]
            .char_indices()
            .find(|(off, ch)| mask[at + off] && !ch.is_whitespace())
            .is_some_and(|(off, _)| text[at + off..].starts_with("->"))
    };
    while i < bytes.len() {
        if !mask[i] {
            // Quotes in comments are trivia, not completed expression operands.
            if bytes[i..].starts_with(b"//") {
                i = text[i..].find('\n').map_or(bytes.len(), |off| i + off);
                continue;
            }
            if bytes[i..].starts_with(b"/*") {
                i = block_comment_end(bytes, i);
                continue;
            }
            if matches!(bytes[i], b'"' | b'\'') {
                expect_operand = false;
            }
            i += 1;
            continue;
        }
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if closure_params {
            match bytes[i] {
                b'(' | b'[' | b'{' => match crate::parameter_object::matching_bracket(text, i) {
                    Some(close) => i = close + 1,
                    None => return bytes.len(),
                },
                b'|' => {
                    closure_params = false;
                    expect_operand = true;
                    closure_ret = has_arrow(i + 1);
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => {
                if bytes[i] == b'{' && angles == 0 {
                    closure_ret = false;
                }
                match crate::parameter_object::matching_bracket(text, i) {
                    Some(close) => i = close + 1,
                    None => return bytes.len(),
                }
                expect_operand = false;
                continue;
            }
            // Consume binary << and <= together. Their second byte cannot start
            // a qualified path. At an operand position, <<T as A>::B as C>::f()
            // instead opens two nested qualified paths.
            b'<' if angles == 0
                && !expect_operand
                && matches!(bytes.get(i + 1), Some(b'<' | b'=')) =>
            {
                expect_operand = true;
                i += 2;
                continue;
            }
            // `<Type as Trait>::item` is an expression operand; a comparison
            // follows a completed operand. Within its type, bare generics also
            // open angles, unlike expression-position generics (turbofish).
            b'<' if expect_operand
                || angles > 0
                || closure_ret
                || turbofish_open(text, from, i, &mask) =>
            {
                angles += 1;
            }
            b'>' if angles > 0 && bytes.get(i.wrapping_sub(1)) != Some(&b'-') => {
                angles -= 1;
                if angles == 0 {
                    expect_operand = false;
                }
            }
            b',' if angles > 0 => {}
            b';' | b',' | b')' | b']' | b'}' => return i,
            b'|' if angles == 0 => {
                if bytes.get(i + 1) == Some(&b'|') {
                    if expect_operand {
                        closure_ret = has_arrow(i + 2);
                    }
                    expect_operand = true;
                    i += 2;
                    continue;
                }
                closure_params = expect_operand;
                expect_operand = true;
            }
            b':' | b'-'
                if bytes.get(i + 1) == Some(if bytes[i] == b':' { &b':' } else { &b'>' }) =>
            {
                expect_operand = false;
                i += 2;
                continue;
            }
            b'?' => expect_operand = false,
            _ if angles == 0 => {
                let ch = text[i..].chars().next().unwrap();
                if is_ident(ch) {
                    let len: usize = text[i..]
                        .chars()
                        .take_while(|c| is_ident(*c))
                        .map(char::len_utf8)
                        .sum();
                    let word = &text[i..i + len];
                    expect_operand = matches!(
                        word,
                        "move"
                            | "async"
                            | "static"
                            | "const"
                            | "return"
                            | "break"
                            | "else"
                            | "in"
                            | "as"
                    );
                    i += len;
                    continue;
                }
                expect_operand = true;
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

fn block_comment_end(bytes: &[u8], mut at: usize) -> usize {
    let mut depth = 1usize;
    at += 2;
    while at < bytes.len() && depth > 0 {
        if bytes[at..].starts_with(b"/*") {
            depth += 1;
            at += 2;
        } else if bytes[at..].starts_with(b"*/") {
            depth -= 1;
            at += 2;
        } else {
            at += 1;
        }
    }
    at
}

fn turbofish_open(text: &str, from: usize, at: usize, mask: &[bool]) -> bool {
    let mut prev = text[from..at]
        .char_indices()
        .rev()
        .filter(|(off, ch)| mask[from + off] && !ch.is_whitespace());
    matches!(
        (prev.next(), prev.next()),
        (Some((second, ':')), Some((first, ':'))) if first + 1 == second
    )
}
