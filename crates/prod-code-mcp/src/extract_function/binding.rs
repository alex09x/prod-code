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

/// Finds local variables and parameters from the enclosing function that are referenced in the
/// extracted helper but are neither declared as parameters nor bound internally (#958).
pub fn uncaptured_enclosing_locals(
    text: &str,
    start: usize,
    extracted: &str,
    function_span: Option<(usize, usize)>,
) -> Vec<String> {
    let Some((body_open, _)) = crate::introduce_variable::enclosing_body(text, start) else {
        return Vec::new();
    };
    let mut enclosing_scope = std::collections::BTreeSet::new();
    if start > body_open {
        for name in bound_names(&text[body_open..start]) {
            enclosing_scope.insert(name);
        }
    }
    if let Some(fn_idx) = text[..body_open].rfind("fn ") {
        if let Some((_, list_open, list_close)) = crate::signature::param_span(text, fn_idx + 3) {
            let param_text = &text[list_open..list_close];
            for param in param_text.split(',') {
                if let Some((name_part, _)) = param.split_once(':') {
                    let name = name_part.trim().trim_start_matches("mut ").trim();
                    if !name.is_empty() && !matches!(name, "self" | "&self" | "&mut self") {
                        enclosing_scope.insert(name.to_string());
                    }
                }
            }
        }
    }

    let (fn_start, fn_end) = match function_span {
        Some((s, e)) => (s, e),
        None => {
            let Some(pos) = extracted.rfind("fn ") else {
                return Vec::new();
            };
            let Some(open) = extracted[pos..].find('{').map(|i| pos + i) else {
                return Vec::new();
            };
            let Some(close) = crate::parameter_object::matching_bracket(extracted, open) else {
                return Vec::new();
            };
            (pos, close + 1)
        }
    };
    if fn_start >= extracted.len() || fn_end > extracted.len() {
        return Vec::new();
    }
    let fn_text = &extracted[fn_start..fn_end];
    let mut declared_params = std::collections::BTreeSet::new();
    if let Some(fn_kw) = fn_text.find("fn ") {
        if let Some((_, list_open, list_close)) = crate::signature::param_span(fn_text, fn_kw + 3) {
            let param_text = &fn_text[list_open..list_close];
            for param in param_text.split(',') {
                if let Some((name_part, _)) = param.split_once(':') {
                    let name = name_part.trim().trim_start_matches("mut ").trim();
                    if !name.is_empty() && !matches!(name, "self" | "&self" | "&mut self") {
                        declared_params.insert(name.to_string());
                    }
                }
            }
        }
    }
    let body_start = fn_text.find('{').map_or(0, |i| i + 1);
    let body = &fn_text[body_start..];
    let internal_locals: std::collections::BTreeSet<String> =
        bound_names(body).into_iter().collect();

    let mut uncaptured = Vec::new();
    for token in super::tokens::tokens(body) {
        if token.0 == super::tokens::Token::Word {
            let word = &body[token.1..token.2];
            if enclosing_scope.contains(word)
                && !declared_params.contains(word)
                && !internal_locals.contains(word)
            {
                uncaptured.push(word.to_string());
            }
        } else if token.0 == super::tokens::Token::Str {
            let str_val = &body[token.1..token.2];
            for word in format_string_interpolations(str_val) {
                if enclosing_scope.contains(&word)
                    && !declared_params.contains(&word)
                    && !internal_locals.contains(&word)
                {
                    uncaptured.push(word);
                }
            }
        }
    }
    uncaptured.sort();
    uncaptured.dedup();
    uncaptured
}

fn format_string_interpolations(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                i += 2;
                continue;
            }
            if let Some(close) = s[i + 1..].find('}') {
                let inside = &s[i + 1..i + 1 + close];
                let var = inside.split(':').next().unwrap_or("").trim();
                let mut chars = var.chars();
                if chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
                    && chars.all(|c| c.is_alphanumeric() || c == '_')
                {
                    out.push(var.to_string());
                }
                i += 1 + close + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}
