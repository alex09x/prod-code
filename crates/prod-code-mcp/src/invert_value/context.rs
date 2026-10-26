/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::extract_trait::lex::lexical_code;

struct Token<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

/// Local context tokens: comments are trivia, literals remain opaque operands. In particular,
/// skipping a string must not turn the binary `"text" == ... & value` into a borrow.
fn tokens(text: &str) -> Vec<Token<'_>> {
    let code = lexical_code(text);
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with("//") && !code[i] {
            i = text[i..].find('\n').map_or(text.len(), |n| i + n);
            continue;
        }
        if text[i..].starts_with("/*") && !code[i] {
            let mut depth = 1;
            i += 2;
            while i < text.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        let ch = text[i..].chars().next().unwrap();
        if ch.is_whitespace() {
            i += ch.len_utf8();
            continue;
        }
        let start = i;
        if !code[i] {
            while i < text.len() && !code[i] {
                i += 1;
            }
        } else if super::types::is_ident(ch) {
            i += ch.len_utf8();
            while i < text.len() && code[i] {
                let ch = text[i..].chars().next().unwrap();
                if !super::types::is_ident(ch) {
                    break;
                }
                i += ch.len_utf8();
            }
        } else {
            i += ch.len_utf8();
        }
        out.push(Token {
            text: &text[start..i],
            start,
            end: i,
        });
    }
    out
}

fn matching_open(tokens: &[Token<'_>], close: usize, left: &str, right: &str) -> Option<usize> {
    let mut depth = 0usize;
    for i in (0..=close).rev() {
        if tokens[i].text == right {
            depth += 1;
        } else if tokens[i].text == left {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Read complete outer attributes attached to this struct, including attributes on its line.
/// Stop at an item boundary; a derive on an earlier struct is not attached to this one.
pub(super) fn derives_above(text: &str, header: usize) -> Vec<String> {
    let tokens = tokens(text);
    let Some(mut end) = tokens
        .iter()
        .position(|t| t.start >= header && t.text == "struct")
    else {
        return Vec::new();
    };
    // Visibility belongs to the item, after its outer attributes.
    if end > 0
        && tokens[end - 1].text == ")"
        && let Some(open) = matching_open(&tokens, end - 1, "(", ")")
        && open > 0
        && tokens[open - 1].text == "pub"
    {
        end = open - 1;
    } else if end > 0 && tokens[end - 1].text == "pub" {
        end -= 1;
    }
    let mut attributes = Vec::new();
    while end > 0 && tokens[end - 1].text == "]" {
        let Some(open) = matching_open(&tokens, end - 1, "[", "]") else {
            break;
        };
        if open == 0 || tokens[open - 1].text != "#" {
            break;
        }
        let inner = &tokens[open + 1..end - 1];
        if inner.first().is_some_and(|t| t.text == "derive")
            && inner.get(1).is_some_and(|t| t.text == "(")
            && inner.last().is_some_and(|t| t.text == ")")
        {
            let names: Vec<String> = inner[2..inner.len() - 1]
                .split(|t| t.text == ",")
                .filter_map(|path| path.last())
                .filter(|t| t.text.chars().all(super::types::is_ident))
                .map(|t| t.text.to_string())
                .collect();
            attributes.push(names);
        }
        end = open - 1;
    }
    attributes.into_iter().rev().flatten().collect()
}

fn ends_operand(token: &str) -> bool {
    if matches!(token, "return" | "break" | "yield" | "else" | "in" | "mut") {
        return false;
    }
    matches!(token, ")" | "]" | "}" | "?")
        || token
            .chars()
            .next()
            .is_some_and(|c| super::types::is_ident(c) || c == '"' || c == '\'')
}

/// Is this complete place, possibly surrounded by grouping parentheses, the operand of a
/// shared or mutable borrow? A borrow of a larger expression does not borrow the place.
pub(super) fn borrowed_place(text: &str, begin: usize, end: usize) -> bool {
    let tokens = tokens(text);
    let mut left = tokens.partition_point(|t| t.end <= begin);
    let mut right = tokens.partition_point(|t| t.start < end);
    while left > 0
        && right < tokens.len()
        && tokens[left - 1].text == "("
        && tokens[right].text == ")"
        && matching_open(&tokens, right, "(", ")") == Some(left - 1)
    {
        // `call(place)` is not a grouping expression, even inside `&call(place)`.
        if left >= 2 && ends_operand(tokens[left - 2].text) {
            break;
        }
        left -= 1;
        right += 1;
    }
    if left > 0 && tokens[left - 1].text == "mut" {
        left -= 1;
    }
    let amp_end = left;
    while left > 0 && tokens[left - 1].text == "&" {
        left -= 1;
    }
    left < amp_end && (left == 0 || !ends_operand(tokens[left - 1].text))
}
