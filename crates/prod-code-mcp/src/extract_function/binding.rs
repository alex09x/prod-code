/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::tokens::is_ident;

#[cfg(test)]
#[path = "binding_tests.rs"]
mod tests;

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
        // A path separator `::` is no type annotation.
        let mut depth = 0i32;
        let bytes = pattern.as_bytes();
        for (j, c) in pattern.char_indices() {
            match c {
                '(' | '{' | '[' | '<' => depth += 1,
                ')' | '}' | ']' | '>' => depth -= 1,
                ':' if depth == 0
                    && bytes.get(j + 1) != Some(&b':')
                    && (j == 0 || bytes[j - 1] != b':') =>
                {
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
            let next = pattern[end..].trim_start();
            let field = next.starts_with(':');
            // A binding is told by its role in the pattern, not its case (#1438): a name that
            // opens a tuple or struct pattern or a path, or ends one, is a type or variant;
            // `let Gross = …` binds `Gross`.
            let path = next.starts_with(['(', '{'])
                || pattern[..s].trim_end().ends_with("::")
                || next.starts_with("::");
            let named = !matches!(word, "mut" | "ref" | "_" | "None")
                && !path
                && !word.starts_with(|c: char| c.is_ascii_digit());
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
        .find(|name| reads_local(after, name))
}

/// Whether the code `after` reads the local `name` before a `let` in the same block binds it
/// again (#1438). Only a name token counts: not a field or method after `.`, a path segment
/// after `::`, a struct field before `:`, or text in a comment or literal. A `let` that binds
/// `name` still reads the earlier one in its initializer, and hides it until its block ends.
fn reads_local(after: &str, name: &str) -> bool {
    use super::tokens::{Token, tokens};
    let toks = tokens(after);
    let text = |i: usize| toks.get(i).map(|&(_, s, e)| &after[s..e]);
    let (mut depth, mut hidden_from) = (0i32, None::<i32>);
    let mut i = 0;
    while i < toks.len() {
        let (kind, ..) = toks[i];
        match text(i) {
            Some("{") => depth += 1,
            Some("}") => {
                depth -= 1;
                if hidden_from.is_some_and(|d| depth < d) {
                    hidden_from = None;
                }
            }
            _ => {}
        }
        if hidden_from.is_some() {
            i += 1;
            continue;
        }
        // Only a `let` statement hides the name for the rest of its block; the binding of an
        // `if let` or `while let` lives in its own body, whose reads all count.
        let statement = i
            .checked_sub(1)
            .and_then(text)
            .is_none_or(|t| matches!(t, ";" | "{" | "}"));
        if kind == Token::Word && text(i) == Some("let") && statement {
            // The pattern up to `=`, `;` or `else`, at this nesting.
            let mut j = i + 1;
            let mut nest = 0i32;
            while let Some(t) = text(j) {
                match t {
                    "(" | "[" | "{" => nest += 1,
                    ")" | "]" | "}" => nest -= 1,
                    "=" | ";" if nest == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            let pattern_end = toks.get(j).map_or(after.len(), |&(_, s, _)| s);
            let start = toks.get(i).map_or(0, |&(_, s, _)| s);
            if bound_names(&after[start..pattern_end])
                .iter()
                .any(|n| n == name)
            {
                // The initializer still reads the earlier binding; after it, the new one.
                let mut k = if text(j) == Some("=") { j + 1 } else { j };
                let mut nest = 0i32;
                while let Some(t) = text(k) {
                    match t {
                        "(" | "[" | "{" => nest += 1,
                        ")" | "]" | "}" => nest -= 1,
                        ";" if nest == 0 => break,
                        _ => {}
                    }
                    if is_read(&toks, after, k, name) {
                        return true;
                    }
                    k += 1;
                }
                hidden_from = Some(depth);
                i = k + 1;
                continue;
            }
            i = j;
            continue;
        }
        if is_read(&toks, after, i, name) {
            return true;
        }
        i += 1;
    }
    false
}

/// Whether token `i` of `text` is the name `name` read as a local.
fn is_read(
    toks: &[(super::tokens::Token, usize, usize)],
    text: &str,
    i: usize,
    name: &str,
) -> bool {
    let at = |k: usize| toks.get(k).map(|&(_, s, e)| &text[s..e]);
    if toks[i].0 != super::tokens::Token::Word || at(i) != Some(name) {
        return false;
    }
    let before = i.checked_sub(1).and_then(at);
    let before2 = i.checked_sub(2).and_then(at);
    let field = before == Some(".") && before2 != Some(".");
    let path = before == Some(":") && before2 == Some(":");
    let after_colon = at(i + 1) == Some(":") && at(i + 2) != Some(":");
    // `Point { gross: 1 }` names a field; `match x { gross: … }` is no Rust, so a name before a
    // single `:` inside braces is a field label, or a type ascription in a pattern.
    !field && !path && !after_colon
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
    let internal_locals = super::patterns::local_bindings(body);

    let mut uncaptured = Vec::new();
    for token in super::tokens::tokens(body) {
        if token.0 == super::tokens::Token::Word {
            let word = &body[token.1..token.2];
            if enclosing_scope.contains(word)
                && !declared_params.contains(word)
                && !internal_locals
                    .iter()
                    .any(|binding| binding.covers(word, token.1))
            {
                uncaptured.push(word.to_string());
            }
        } else if token.0 == super::tokens::Token::Str {
            let str_val = &body[token.1..token.2];
            for word in format_string_interpolations(str_val) {
                if enclosing_scope.contains(&word)
                    && !declared_params.contains(&word)
                    && !internal_locals
                        .iter()
                        .any(|binding| binding.covers(&word, token.1))
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
