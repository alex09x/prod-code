/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::find_matching_open_paren;

pub(crate) fn is_ordinary_js_block(text: &str, open_pos: usize) -> bool {
    let before = text[..open_pos].trim_end();
    if before.is_empty() {
        return true;
    }
    if before.ends_with(';') || before.ends_with('{') || before.ends_with('}') {
        return true;
    }
    let last_word = before
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next_back()
        .unwrap_or("");
    if matches!(last_word, "else" | "do" | "try" | "finally") {
        return true;
    }
    if before.ends_with(')') {
        if let Some(open_paren) = find_matching_open_paren(text, before.len() - 1) {
            let before_paren = text[..open_paren].trim_end();
            let kw = before_paren
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .next_back()
                .unwrap_or("");
            return matches!(kw, "if" | "for" | "while" | "switch" | "catch");
        }
    }
    false
}

pub(crate) fn strip_function_blocks(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text.as_bytes()[i] == b'{' {
            if !is_ordinary_js_block(text, i) {
                if let Some(close) = crate::parameter_object::matching_bracket(text, i) {
                    i = close + 1;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap();
        result.push(ch);
        i += ch.len_utf8();
    }
    result
}
