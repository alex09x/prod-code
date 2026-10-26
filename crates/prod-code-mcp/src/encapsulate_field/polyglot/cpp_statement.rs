/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Only a control header's closing parenthesis introduces an unbraced body.
/// A cast, call, or parenthesized expression must not grant the same permission.
pub(super) fn body_boundary(text: &str, start: usize, mask: &[bool]) -> bool {
    let tokens: Vec<_> = text[..start]
        .char_indices()
        .filter(|(at, ch)| mask[*at] && !ch.is_whitespace())
        .collect();
    if tokens.last().is_some_and(|(_, ch)| *ch == ')') {
        let mut depth = 0;
        for (i, (_, ch)) in tokens.iter().enumerate().rev() {
            match ch {
                ')' => depth += 1,
                '(' => {
                    depth -= 1;
                    if depth == 0 {
                        let (word, before) = preceding_word(text, &tokens[..i]);
                        let keyword = if word == "constexpr" {
                            preceding_word(text, &tokens[..before]).0
                        } else {
                            word
                        };
                        return matches!(keyword, "if" | "for" | "while" | "switch");
                    }
                }
                _ => {}
            }
        }
        return false;
    }
    matches!(preceding_word(text, &tokens).0, "else" | "do")
}

fn preceding_word<'a>(text: &'a str, tokens: &[(usize, char)]) -> (&'a str, usize) {
    let mut begin = tokens.len();
    let Some(&(last, ch)) = tokens.last() else {
        return ("", begin);
    };
    let mut next = last + ch.len_utf8();
    while begin > 0 {
        let (at, ch) = tokens[begin - 1];
        if !super::super::case::is_ident(ch) || at + ch.len_utf8() != next {
            break;
        }
        next = at;
        begin -= 1;
    }
    (&text[next..last + ch.len_utf8()], begin)
}
