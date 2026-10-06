/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::tokens::{is_ident, mentions};

/// The names the `let` statements in `code` bind: `let x`, `let mut x`, and the names in a
/// tuple or struct pattern (`let (a, b)`, `let P { x, y: py }` binds `x` and `py`).
pub fn bound_names(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in code.match_indices("let") {
        let whole = !code[..i].chars().next_back().is_some_and(is_ident)
            && code[i + 3..].starts_with(char::is_whitespace);
        if !whole {
            continue;
        }
        let rest = &code[i + 3..];
        let pattern_end = rest.find(['=', ';']).unwrap_or(rest.len());
        let mut pattern = &rest[..pattern_end];
        // `let x: T`: the type is not part of the pattern. A `:` inside braces is a field.
        let mut depth = 0i32;
        for (j, c) in pattern.char_indices() {
            match c {
                '(' | '{' | '[' | '<' => depth += 1,
                ')' | '}' | ']' | '>' => depth -= 1,
                ':' if depth == 0 => {
                    pattern = &pattern[..j];
                    break;
                }
                _ => {}
            }
        }
        let chars: Vec<(usize, char)> = pattern.char_indices().collect();
        let mut k = 0;
        while k < chars.len() {
            let (s, c) = chars[k];
            if !is_ident(c) {
                k += 1;
                continue;
            }
            let mut e = k;
            while e < chars.len() && is_ident(chars[e].1) {
                e += 1;
            }
            let end = chars.get(e).map_or(pattern.len(), |(i, _)| *i);
            let word = &pattern[s..end];
            let field = pattern[end..].trim_start().starts_with(':');
            let named = !matches!(word, "mut" | "ref" | "_")
                && !word.starts_with(|c: char| c.is_uppercase() || c.is_ascii_digit());
            if named && !field {
                out.push(word.to_string());
            }
            k = e;
        }
    }
    out.sort();
    out.dedup();
    out
}

/// A name the selection binds, the call does not bind again, and the code after the place
/// ending at `to` in its function reads (#189). Such a place cannot take the call: the read
/// would find an outer binding of the name, or none, and only the second is an error the
/// analyzer reports.
pub fn read_after_but_not_returned(
    text: &str,
    selection: &str,
    call: &str,
    to: usize,
) -> Option<String> {
    let returned = bound_names(call);
    let (_, close) = crate::introduce_variable::enclosing_body(text, to.saturating_sub(1))?;
    let after = &text[to.min(close)..close];
    bound_names(selection)
        .into_iter()
        .filter(|name| !returned.contains(name))
        .find(|name| mentions(after, name))
}
