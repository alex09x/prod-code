/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature_go::syntax::canonical;
use crate::signature_go::text::{
    closing, is_ident_byte, skip_opaque, skip_space, split_list, strip_comments,
};
use crate::signature_go::types::{Decl, GoParam};

/// The declarations at the top level of a Go file, in order.
pub(crate) fn declarations(text: &str) -> Vec<Decl> {
    let s = text.as_bytes();
    let mut out = Vec::new();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'f' if depth == 0
                && text[i..].starts_with("func")
                && (i == 0 || !is_ident_byte(s[i - 1]))
                && !s.get(i + 4).is_some_and(|b| is_ident_byte(*b)) =>
            {
                if let Some(d) = header(text, i) {
                    i = d.close + 1;
                    out.push(d);
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The declaration whose `func` keyword is at `func_at`: `func (r T) Name[P any](…) results`.
/// `None` for a function literal or type, which has no name.
pub(crate) fn header(text: &str, func_at: usize) -> Option<Decl> {
    let s = text.as_bytes();
    if !text[func_at..].starts_with("func") {
        return None;
    }
    let mut i = skip_space(text, func_at + 4);
    let mut receiver = None;
    if s.get(i) == Some(&b'(') {
        let close = closing(text, i)?;
        receiver = Some(canonical(&text[i + 1..close]));
        i = skip_space(text, close + 1);
    }
    let name_at = i;
    while i < s.len() && is_ident_byte(s[i]) {
        i += 1;
    }
    if i == name_at || s[name_at].is_ascii_digit() {
        return None;
    }
    let name = text[name_at..i].to_string();
    i = skip_space(text, i);
    let mut generic = false;
    if s.get(i) == Some(&b'[') {
        generic = true;
        i = skip_space(text, closing(text, i)? + 1);
    }
    if s.get(i) != Some(&b'(') {
        return None;
    }
    let open = i;
    let close = closing(text, open)?;
    let end = body_open(text, close + 1).unwrap_or_else(|| {
        text[close + 1..]
            .find('\n')
            .map_or(text.len(), |n| close + 1 + n)
    });
    Some(Decl {
        func_at,
        name,
        name_at,
        receiver,
        generic,
        open,
        close,
        results: canonical(&text[close + 1..end]),
    })
}

/// The `{` that opens the body after a signature's parameters, skipping the braces of a
/// `struct{…}` or `interface{…}` result; `None` at the end of the line without one.
pub(crate) fn body_open(text: &str, from: usize) -> Option<usize> {
    let s = text.as_bytes();
    let mut i = from;
    while i < s.len() {
        if s[i] == b'/' && s.get(i + 1) == Some(&b'/') {
            return None;
        }
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'\n' | b';' => return None,
            b'{' => {
                let before = text[from..i].trim_end();
                if before.ends_with("struct") || before.ends_with("interface") {
                    i = closing(text, i)? + 1;
                    continue;
                }
                return Some(i);
            }
            b'(' | b'[' => {
                i = closing(text, i)? + 1;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The parameters of a declaration's list, flattened; an error for a list gopls cannot reorder
/// by name (unnamed or blank parameters).
pub(crate) fn parameters(list: &str) -> std::result::Result<Vec<GoParam>, String> {
    let mut pending: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut named = false;
    for piece in split_list(&strip_comments(list)) {
        let word_end = piece
            .bytes()
            .position(|b| !is_ident_byte(b))
            .unwrap_or(piece.len());
        let word = &piece[..word_end];
        let rest = piece[word_end..].trim();
        let keyword = matches!(word, "chan" | "func" | "map" | "struct" | "interface");
        let spaced = piece[word_end..].starts_with(|c: char| c.is_whitespace());
        if !word.is_empty() && !keyword && spaced && !rest.is_empty() {
            named = true;
            let ty = canonical(rest);
            for name in pending.drain(..) {
                out.push(GoParam {
                    name,
                    ty: ty.clone(),
                });
            }
            out.push(GoParam {
                name: word.to_string(),
                ty,
            });
        } else if !word.is_empty() && word_end == piece.len() && !keyword {
            pending.push(word.to_string());
        } else {
            return Err(format!("`{piece}` is an unnamed parameter"));
        }
    }
    if !pending.is_empty() {
        return Err(if named {
            format!("`{}` has no type", pending.join(", "))
        } else {
            "its parameters are unnamed".to_string()
        });
    }
    if out.iter().any(|p| p.name == "_") {
        return Err("it has a blank `_` parameter".to_string());
    }
    Ok(out)
}

/// The parentheses of the call whose callee's name starts at `at` (`f(…)`, `x.f(…)`,
/// `f[T](…)`); `None` when the name is not called there.
pub(crate) fn call_parens(text: &str, at: usize) -> Option<(usize, usize)> {
    let s = text.as_bytes();
    let inline = |mut i: usize| {
        while i < s.len() && matches!(s[i], b' ' | b'\t') {
            i += 1;
        }
        i
    };
    let mut i = at;
    while i < s.len() && is_ident_byte(s[i]) {
        i += 1;
    }
    i = inline(i);
    if s.get(i) == Some(&b'[') {
        i = inline(closing(text, i)? + 1);
    }
    if s.get(i) != Some(&b'(') {
        return None;
    }
    Some((i, closing(text, i)?))
}

/// Where each flattened parameter's name starts in the list between `open` and `close`, in the
/// order [`parameters`] gives them: the first word of every piece between top-level commas.
pub(crate) fn parameter_names_at(text: &str, open: usize, close: usize) -> Vec<usize> {
    let s = text.as_bytes();
    let mut starts = vec![open + 1];
    let (mut depth, mut i) = (0i32, open + 1);
    while i < close {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => starts.push(i + 1),
            _ => {}
        }
        i += 1;
    }
    starts
        .into_iter()
        .map(|start| skip_space(text, start))
        .filter(|&at| at < close && is_ident_byte(s[at]))
        .collect()
}

/// The whole identifier that starts at `at`, if one does.
pub(crate) fn ident_at(text: &str, at: usize) -> Option<&str> {
    let s = text.as_bytes();
    if at >= s.len() || !is_ident_byte(s[at]) || (at > 0 && is_ident_byte(s[at - 1])) {
        return None;
    }
    let end = s[at..]
        .iter()
        .position(|&b| !is_ident_byte(b))
        .map_or(s.len(), |n| at + n);
    Some(&text[at..end])
}

/// Where `name` is written as a word in `text[from..to]`, strings and comments aside, unless it
/// follows a dot (a field, a method or a package member, never a local). Anything else counts,
/// a struct literal's key or a label too: this is the proof that a name is unused, and it errs
/// towards a use.
pub(crate) fn identifier_uses(text: &str, from: usize, to: usize, name: &str) -> Vec<usize> {
    let s = text.as_bytes();
    let mut out = Vec::new();
    let mut i = from;
    while i < to {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        if let Some(word) = ident_at(text, i) {
            if word == name && !text[..i].trim_end().ends_with('.') {
                out.push(i);
            }
            i += word.len();
            continue;
        }
        i += 1;
    }
    out
}
