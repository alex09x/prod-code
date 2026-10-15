/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn mentions(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        let before_ok = at == 0 || !is_ident(text[..at].chars().last().unwrap());
        let after = at + name.len();
        let after_ok = after == text.len() || !is_ident(text[after..].chars().next().unwrap());
        before_ok && after_ok
    })
}

/// What a token of Rust source is, as far as matching copies needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Word,
    Number,
    Str,
    Char,
    Punct,
}

/// The tokens of `text`, as (kind, start, end): names and keywords, number, string and char
/// literals, and every other character on its own. Whitespace and `//` comments are skipped.
pub fn tokens(text: &str) -> Vec<(Token, usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = text[i..].chars().next().unwrap_or(' ');
        let len = c.len_utf8();
        if c.is_whitespace() {
            i += len;
        } else if text[i..].starts_with("//") {
            i = text[i..].find('\n').map_or(text.len(), |n| i + n);
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'.')
            {
                // `1..3` is two numbers and a range, not one number.
                if bytes[j] == b'.' && bytes.get(j + 1) == Some(&b'.') {
                    break;
                }
                j += 1;
            }
            out.push((Token::Number, i, j));
            i = j;
        } else if c == '"' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'"' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let j = (j + 1).min(bytes.len());
            out.push((Token::Str, i, j));
            i = j;
        } else if c == '\'' {
            // A char literal closes within a few bytes; a lifetime does not close at all.
            let close = text[i + 1..]
                .char_indices()
                .take(4)
                .find(|(k, ch)| *ch == '\'' && *k > 0)
                .map(|(k, _)| i + 1 + k);
            match close {
                Some(end)
                    if !text[i + 1..end].starts_with(|ch: char| ch.is_alphabetic())
                        || end - i <= 3 =>
                {
                    out.push((Token::Char, i, end + 1));
                    i = end + 1;
                }
                _ => {
                    let mut j = i + 1;
                    while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
                    {
                        j += 1;
                    }
                    out.push((Token::Word, i, j));
                    i = j;
                }
            }
        } else if is_ident(c) {
            let mut j = i;
            while let Some(ch) = text[j..].chars().next() {
                if !is_ident(ch) {
                    break;
                }
                j += ch.len_utf8();
            }
            out.push((Token::Word, i, j));
            i = j;
        } else {
            out.push((Token::Punct, i, i + len));
            i += len;
        }
    }
    out
}

use std::collections::{BTreeMap, BTreeSet};

use crate::signature::call_sites::{normalize_type_spelling, opaque_at};

#[derive(Debug, Clone)]
pub(crate) struct ActiveBinding {
    pub is_mut: bool,
    pub is_ref_pat: bool,
    pub explicit_ty: Option<String>,
    pub rhs: String,
    pub stmt_start: usize,
    pub stmt_end: usize,
}

pub(crate) fn is_field_path_of(expr: &str, parent: &str) -> bool {
    let expr = expr.trim().trim_matches(['(', ')']).trim();
    expr == parent
        || expr
            .strip_prefix(parent)
            .and_then(|r| r.strip_prefix('.'))
            .is_some_and(|r| {
                !r.is_empty()
                    && r.split('.')
                        .all(|s| !s.is_empty() && s.chars().all(is_ident))
            })
}

/// Parses unique top-level nominal `struct` definitions (`struct_name -> { field -> norm_ty }`)
/// and collects any type names that are multiply defined or imported/aliased in `text`.
pub(crate) fn parse_unique_structs(
    text: &str,
) -> (BTreeMap<String, BTreeMap<String, String>>, BTreeSet<String>) {
    let bytes = text.as_bytes();
    let mut structs: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < bytes.len() {
        if let Ok(Some((end, _))) = opaque_at(text, i, 0) {
            i = end;
            continue;
        }
        match bytes[i] {
            b'{' => {
                depth += 1;
                i += 1;
                continue;
            }
            b'}' => {
                depth = (depth - 1).max(0);
                i += 1;
                continue;
            }
            _ => {}
        }
        let prev_id = text[..i].chars().next_back().is_some_and(is_ident);
        if !prev_id
            && text[i..].starts_with("use")
            && text[i + 3..].starts_with(char::is_whitespace)
            && let Some(semi) = text[i + 3..].find(';')
        {
            for tok in text[i + 3..i + 3 + semi].split(|c: char| !is_ident(c)) {
                if tok.chars().next().is_some_and(char::is_uppercase) {
                    *counts.entry(tok.to_string()).or_insert(0) += 2;
                }
            }
            i += 3 + semi + 1;
            continue;
        }
        if !prev_id {
            for kw in ["struct", "enum", "union", "type"] {
                if text[i..].starts_with(kw)
                    && text[i + kw.len()..].starts_with(char::is_whitespace)
                {
                    let rest = text[i + kw.len()..].trim_start();
                    let name_len = rest
                        .chars()
                        .take_while(|c| is_ident(*c))
                        .map(char::len_utf8)
                        .sum();
                    if name_len > 0 {
                        let name = &rest[..name_len];
                        *counts.entry(name.to_string()).or_insert(0) += 1;
                        let after_name = rest[name_len..].trim_start();
                        if kw == "struct"
                            && depth == 0
                            && after_name.starts_with('{')
                            && let Some(close) =
                                crate::parameter_object::matching_bracket(after_name, 0)
                        {
                            let mut fields = BTreeMap::new();
                            for item in after_name[1..close].split(',') {
                                if let Some((lhs, ty)) =
                                    crate::signature::parse::split_at_top_level(item, ':')
                                    && let Some(fname) = lhs
                                        .split_whitespace()
                                        .last()
                                        .filter(|f| f.chars().all(is_ident))
                                {
                                    fields.insert(fname.to_string(), normalize_type_spelling(ty));
                                }
                            }
                            structs.insert(name.to_string(), fields);
                        }
                    }
                    break;
                }
            }
        }
        i += text[i..].chars().next().map_or(1, char::len_utf8);
    }
    let shadowed: BTreeSet<String> = counts
        .into_iter()
        .filter(|(_, c)| *c > 1)
        .map(|(k, _)| k)
        .collect();
    for s in &shadowed {
        structs.remove(s);
    }
    (structs, shadowed)
}

fn parse_let_stmt(rest: &str) -> Option<(Option<usize>, usize)> {
    let (mut depth, mut eq_at, mut j) = (0i32, None, 0usize);
    while j < rest.len() {
        if let Ok(Some((end, _))) = opaque_at(rest, j, 0) {
            j = end;
            continue;
        }
        match rest.as_bytes()[j] {
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' if depth == 0 => return None,
            b')' | b'}' | b']' => depth -= 1,
            b'=' if depth == 0
                && eq_at.is_none()
                && !rest[..j].ends_with(['=', '!', '<', '>'])
                && !rest[j + 1..].starts_with(['=', '>']) =>
            {
                eq_at = Some(j);
            }
            b';' if depth == 0 => return Some((eq_at, j)),
            _ => {}
        }
        j += rest[j..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

/// Scope-aware scan of `let` bindings in `code` that remain in scope at the end of `code`.
pub(crate) fn active_let_bindings(code: &str) -> BTreeMap<String, ActiveBinding> {
    let bytes = code.as_bytes();
    let mut scopes: Vec<BTreeMap<String, Option<ActiveBinding>>> = vec![BTreeMap::new()];
    let mut pending_shadows: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Ok(Some((end, _))) = opaque_at(code, i, 0) {
            i = end;
            continue;
        }
        if bytes[i] == b'{' {
            let d = scopes.len();
            let mut frame = BTreeMap::new();
            for (_, ident) in pending_shadows.iter().filter(|(pd, _)| *pd == d) {
                frame.insert(ident.clone(), None);
            }
            scopes.push(frame);
            i += 1;
            continue;
        }
        if bytes[i] == b'}' {
            if scopes.len() > 1 {
                scopes.pop();
            }
            let d = scopes.len();
            pending_shadows.retain(|(pd, _)| *pd <= d);
            i += 1;
            continue;
        }
        let prev_id = code[..i].chars().next_back().is_some_and(is_ident);
        if !prev_id
            && code[i..].starts_with("for")
            && code[i + 3..].starts_with(char::is_whitespace)
            && let Some(in_pos) = code[i + 3..]
                .find(" in ")
                .or_else(|| code[i + 3..].find('\n'))
        {
            let d = scopes.len();
            for tok in code[i + 3..i + 3 + in_pos].split(|c: char| !is_ident(c)) {
                if !tok.is_empty() {
                    pending_shadows.push((d, tok.to_string()));
                }
            }
        }
        if !prev_id
            && code[i..].starts_with("let")
            && code[i + 3..].starts_with(char::is_whitespace)
        {
            let before = code[..i].trim_end();
            let is_cond = ["if", "while"].iter().any(|kw| {
                before.ends_with(kw)
                    && !before[..before.len() - kw.len()]
                        .chars()
                        .next_back()
                        .is_some_and(is_ident)
            });
            let rest = &code[i + 3..];
            if is_cond {
                if let Some(eq) = rest.find('=') {
                    let d = scopes.len();
                    for tok in rest[..eq]
                        .split(|c: char| !is_ident(c))
                        .filter(|s| !s.is_empty())
                    {
                        pending_shadows.push((d, tok.to_string()));
                    }
                }
            } else if let Some((eq_at, semi)) = parse_let_stmt(rest) {
                let header = rest[..eq_at.unwrap_or(semi)].trim();
                let split = crate::signature::parse::split_at_top_level(header, ':');
                let (pat, explicit_ty) = split.map_or((header, None), |(p, ty)| {
                    (p.trim(), Some(ty.trim().to_string()))
                });
                let (is_ref_pat, after_ref) = pat
                    .strip_prefix("ref")
                    .filter(|r| r.starts_with(char::is_whitespace))
                    .map_or((false, pat), |r| (true, r.trim_start()));
                let (is_mut, name) = after_ref
                    .strip_prefix("mut")
                    .filter(|r| r.starts_with(char::is_whitespace))
                    .map_or((false, after_ref.trim()), |r| (true, r.trim()));
                let frame = scopes.last_mut().unwrap();
                if !name.is_empty() && name.chars().all(is_ident) {
                    let entry = eq_at
                        .map(|eq| rest[eq + 1..semi].trim())
                        .filter(|rhs| !rhs.is_empty())
                        .map(|rhs| ActiveBinding {
                            is_mut,
                            is_ref_pat,
                            explicit_ty,
                            rhs: rhs.to_string(),
                            stmt_start: i,
                            stmt_end: i + 3 + semi + 1,
                        });
                    frame.insert(name.to_string(), entry);
                } else {
                    for tok in pat.split(|c: char| !is_ident(c)).filter(|s| !s.is_empty()) {
                        frame.insert(tok.to_string(), None);
                    }
                }
                i = i + 3 + semi + 1;
                continue;
            }
        }
        i += code[i..].chars().next().map_or(1, char::len_utf8);
    }
    let mut out = BTreeMap::new();
    for (k, v) in scopes.into_iter().flatten() {
        if let Some(b) = v {
            out.insert(k, b);
        } else {
            out.remove(&k);
        }
    }
    out
}
