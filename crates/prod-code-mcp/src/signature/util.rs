/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

use crate::signature::call_sites::opaque_at;

/// The text of a file the change reads, a caller or one it rewrites. A file that cannot be read
/// stops the change: read as empty, its calls would be neither checked nor rewritten.
pub fn read_caller(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("cannot read {}; nothing was written", path.display()))
}

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// A parameter list on one line, for the report.
pub fn normalize(list: &str) -> String {
    let one_line = list.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line.trim_end_matches(',').to_string()
}

/// 1-based (line, UTF-16 column) of byte `offset` of `text`, as LSP positions count it. An offset
/// past the end of the text, inside a character, or between the `\r` and the `\n` of a line
/// break is on no position: `None`, not the nearest one.
pub fn line_col_at(text: &str, offset: usize) -> Option<(u32, u32)> {
    if !text.is_char_boundary(offset) {
        return None;
    }
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |n| n + 1);
    let in_line = &before[line_start..];
    if in_line.ends_with('\r') && text[offset..].starts_with('\n') {
        return None;
    }
    let line = u32::try_from(before.matches('\n').count()).ok()?;
    let col = u32::try_from(in_line.encode_utf16().count()).ok()?;
    Some((line.checked_add(1)?, col.checked_add(1)?))
}

/// [`line_col_at`] for a planner, whose offsets come from the text itself: one that is on no
/// position is an error that stops the plan.
pub fn position_at(text: &str, offset: usize) -> Result<(u32, u32)> {
    line_col_at(text, offset).with_context(|| {
        format!(
            "byte {offset} of a {}-byte file is on no line and column; nothing was planned",
            text.len()
        )
    })
}

/// Normalizes a Rust type annotation for exact parameter comparison, removing lifetime tokens and
/// redundant whitespace while preserving `mut` and path structure.
pub fn normalize_type_spelling(ty: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = ty.chars().collect();
    let mut i = 0;
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    while i < chars.len() {
        if chars[i] == '\'' && i + 1 < chars.len() && is_id(chars[i + 1]) {
            let mut j = i + 1;
            while j < chars.len() && is_id(chars[j]) {
                j += 1;
            }
            if j >= chars.len() || chars[j] != '\'' {
                i = j;
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                continue;
            }
        }
        if chars[i].is_whitespace() {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i < chars.len() && out.chars().next_back().is_some_and(is_id) && is_id(chars[i]) {
                out.push(' ');
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Explicitly typed `let` bindings (`let v: T = …;`) in an open lexical scope before `at`.
/// Closed inner blocks, comments, and literals are ignored; ordinary `let` bindings activate only
/// after their terminating semicolon, and untyped, destructuring, `for`, `if/while let`, closure,
/// `match` arm, or inner `fn` bindings shadow earlier bindings in outer scopes.
pub fn typed_locals_before(text: &str, at: usize) -> BTreeMap<String, String> {
    let body_start = crate::introduce_variable::enclosing_body(text, at)
        .map_or(0, |(open, _)| (open + 1).min(at));
    let scope = &text[body_start..at];
    let bytes = scope.as_bytes();
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut scopes: Vec<BTreeMap<String, Option<String>>> = vec![BTreeMap::new()];
    let mut pending_shadows: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Ok(Some((end, _))) = opaque_at(scope, i, 0) {
            i = end;
            continue;
        }
        match bytes[i] {
            b'{' => {
                let d = scopes.len();
                let mut frame = BTreeMap::new();
                for (_, ident) in pending_shadows.iter().filter(|(pd, _)| *pd == d) {
                    frame.insert(ident.clone(), None);
                }
                scopes.push(frame);
                i += 1;
                continue;
            }
            b'}' => {
                if scopes.len() > 1 {
                    scopes.pop();
                }
                let d = scopes.len();
                pending_shadows.retain(|(pd, _)| *pd <= d);
                i += 1;
                continue;
            }
            b';' => {
                let d = scopes.len();
                pending_shadows.retain(|(pd, _)| *pd < d);
                i += 1;
                continue;
            }
            b'|' if bytes.get(i + 1) != Some(&b'|')
                && bytes.get(i + 1) != Some(&b'=')
                && (i == 0 || bytes[i - 1] != b'|') =>
            {
                if let Some(close) = scope[i + 1..].find(['|', ';', '{'])
                    && scope.as_bytes()[i + 1 + close] == b'|'
                {
                    for tok in scope[i + 1..i + 1 + close].split(|c: char| !is_id(c)) {
                        if !tok.is_empty() {
                            scopes.last_mut().unwrap().insert(tok.to_string(), None);
                        }
                    }
                    i += close + 2;
                    continue;
                }
            }
            b'=' if bytes.get(i + 1) == Some(&b'>') => {
                let pat_start = scope[..i].rfind(['{', ',', '}', ';']).map_or(0, |p| p + 1);
                for tok in scope[pat_start..i].split(|c: char| !is_id(c)) {
                    if !tok.is_empty() {
                        scopes.last_mut().unwrap().insert(tok.to_string(), None);
                    }
                }
                i += 2;
                continue;
            }
            _ => {}
        }
        let prev_id = scope[..i].chars().next_back().is_some_and(is_id);
        if !prev_id
            && scope[i..].starts_with("for")
            && scope[i + 3..].starts_with(char::is_whitespace)
            && let Some(in_pos) = scope[i + 3..]
                .find(" in ")
                .or_else(|| scope[i + 3..].find('\n'))
        {
            let d = scopes.len();
            for tok in scope[i + 3..i + 3 + in_pos].split(|c: char| !is_id(c)) {
                if !tok.is_empty() {
                    pending_shadows.push((d, tok.to_string()));
                }
            }
        }
        if !prev_id
            && scope[i..].starts_with("fn")
            && scope[i + 2..].starts_with(char::is_whitespace)
            && let Some((_, open, close)) = crate::signature::param_span(scope, i + 2)
        {
            let d = scopes.len();
            for tok in scope[open..close].split(|c: char| !is_id(c)) {
                if !tok.is_empty() {
                    pending_shadows.push((d, tok.to_string()));
                }
            }
        }
        if !prev_id
            && scope[i..].starts_with("let")
            && scope[i + 3..].starts_with(char::is_whitespace)
        {
            let before = scope[..i].trim_end();
            let cond_let = ["if", "while"].iter().any(|kw| {
                before.ends_with(kw)
                    && !before[..before.len() - kw.len()]
                        .chars()
                        .next_back()
                        .is_some_and(is_id)
            });
            let rest = &scope[i + 3..];
            let (mut depth, mut eq_at, mut semi_at, mut brace_at, mut j) =
                (0i32, None, None, None, 0usize);
            while j < rest.len() {
                if let Ok(Some((next, _))) = opaque_at(rest, j, 0) {
                    j = next;
                    continue;
                }
                match rest.as_bytes()[j] {
                    b'(' | b'[' => depth += 1,
                    b')' | b']' => depth = (depth - 1).max(0),
                    b'{' if depth == 0 && eq_at.is_none() => {
                        brace_at = Some(j);
                        break;
                    }
                    b'{' => depth += 1,
                    b'}' => depth = (depth - 1).max(0),
                    b'=' if depth == 0
                        && eq_at.is_none()
                        && !rest[..j].ends_with(['=', '!', '<', '>'])
                        && !rest[j + 1..].starts_with(['=', '>']) =>
                    {
                        eq_at = Some(j);
                        if cond_let {
                            break;
                        }
                    }
                    b';' if depth == 0 => {
                        semi_at = Some(j);
                        break;
                    }
                    _ => {}
                }
                j += rest[j..].chars().next().map_or(1, char::len_utf8);
            }
            let hdr_end = eq_at.or(semi_at).or(brace_at).unwrap_or(rest.len());
            let header = rest[..hdr_end].trim();
            if cond_let {
                let d = scopes.len();
                for tok in header.split(|c: char| !is_id(c)).filter(|s| !s.is_empty()) {
                    pending_shadows.push((d, tok.to_string()));
                }
                i += 3 + eq_at.map_or(hdr_end, |eq| eq + 1);
                continue;
            }
            let Some(semi) = semi_at else {
                i += 3 + eq_at.map_or(hdr_end, |eq| eq + 1);
                continue;
            };
            pending_shadows.retain(|(pd, _)| *pd < scopes.len());
            if let Some((pat, ty)) = crate::signature::parse::split_at_top_level(header, ':') {
                let pat = pat.trim();
                let name = pat
                    .strip_prefix("mut")
                    .filter(|r| r.starts_with(char::is_whitespace))
                    .map_or(pat, str::trim);
                if crate::signature::parse::is_ident(name) {
                    scopes
                        .last_mut()
                        .unwrap()
                        .insert(name.to_string(), Some(normalize_type_spelling(ty)));
                } else {
                    for tok in pat.split(|c: char| !is_id(c)).filter(|s| !s.is_empty()) {
                        scopes.last_mut().unwrap().insert(tok.to_string(), None);
                    }
                }
            } else {
                for tok in header.split(|c: char| !is_id(c)).filter(|s| !s.is_empty()) {
                    scopes.last_mut().unwrap().insert(tok.to_string(), None);
                }
            }
            i += 3 + semi + 1;
            continue;
        }
        i += scope[i..].chars().next().map_or(1, char::len_utf8);
    }
    let mut out = BTreeMap::new();
    for (k, v) in scopes.into_iter().flatten() {
        if let Some(ty) = v {
            out.insert(k, ty);
        } else {
            out.remove(&k);
        }
    }
    out
}
