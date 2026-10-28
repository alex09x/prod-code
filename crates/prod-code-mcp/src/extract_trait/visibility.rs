/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::lex::{lexical_code, matching_close_brace, next_code};
use super::super::types::is_ident;

/// The visibility a declaration starts with (`pub`, `pub(crate)`, …; empty when private) and
/// the declaration without it.
pub fn visibility(decl: &str) -> (&str, &str) {
    if let Some(rest) = decl.strip_prefix("pub") {
        let code = lexical_code(decl);
        let next = next_code(decl, &code, 3).unwrap_or(decl.len());
        if decl.as_bytes().get(next) == Some(&b'(') {
            let close = decl[next + 1..].char_indices().find_map(|(offset, c)| {
                let at = next + 1 + offset;
                (code[at] && c == ')').then_some(at)
            });
            if let Some(close) = close {
                let after = next_code(decl, &code, close + 1).unwrap_or(decl.len());
                return (&decl[..=close], &decl[after..]);
            }
        }
        if rest.starts_with(char::is_whitespace) || rest.starts_with("/*") || rest.starts_with("//")
        {
            return ("pub", &decl[next..]);
        }
    }
    ("", decl)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    Private,
    Super(usize),
    CratePath(Vec<String>),
    Crate,
    Pub,
}

fn parse_scope(vis: &str) -> Scope {
    // Comments separate tokens, including nested comments and ')' inside comments.
    let code = lexical_code(vis);
    let masked: String = vis
        .char_indices()
        .map(|(i, c)| if code[i] { c } else { ' ' })
        .collect();
    let vis = masked.as_str();
    let vis = vis.trim();
    if vis.is_empty() {
        return Scope::Private;
    }
    if vis == "pub" {
        return Scope::Pub;
    }
    let Some(inner) = vis
        .strip_prefix("pub")
        .map(str::trim_start)
        .and_then(|s| s.strip_prefix('('))
        .and_then(|s| s.strip_suffix(')'))
    else {
        return Scope::Private;
    };
    let inner = inner.trim();
    let path = inner
        .strip_prefix("in")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map_or(inner, str::trim_start);
    let path = path.strip_prefix("::").map_or(path, str::trim_start);
    let segs: Vec<&str> = path
        .split("::")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    match segs.as_slice() {
        [] | ["self"] => Scope::Private,
        ["crate"] => Scope::Crate,
        all if all.iter().all(|s| *s == "super") => Scope::Super(all.len()),
        ["crate", rest @ ..] => Scope::CratePath(rest.iter().map(|s| (*s).to_string()).collect()),
        other => Scope::CratePath(other.iter().map(|s| (*s).to_string()).collect()),
    }
}

fn common_prefix_scope(a: &[String], b: &[String]) -> Scope {
    let prefix: Vec<String> = a
        .iter()
        .zip(b.iter())
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| x.clone())
        .collect();
    if prefix.is_empty() {
        Scope::Crate
    } else {
        Scope::CratePath(prefix)
    }
}

#[derive(Clone)]
struct InlineModuleScope {
    open: usize,
    close: usize,
    name: String,
}

fn inline_module_scopes(text: &str) -> Vec<InlineModuleScope> {
    let code = lexical_code(text);
    let mut mods = Vec::new();
    for (i, _) in text.match_indices("mod") {
        if !code[i..i + 3].iter().all(|is_code| *is_code)
            || text[..i].chars().next_back().is_some_and(is_ident)
            || text[i + 3..].chars().next().is_some_and(is_ident)
        {
            continue;
        }
        let Some(name_start) = next_code(text, &code, i + 3) else {
            continue;
        };
        let raw_trimmed = text[name_start..]
            .strip_prefix("r#")
            .map_or(name_start, |_| name_start + 2);
        let name: String = text[raw_trimmed..]
            .chars()
            .take_while(|c| is_ident(*c))
            .collect();
        if name.is_empty() {
            continue;
        }
        let after_name = raw_trimmed + name.len();
        let Some(open) = next_code(text, &code, after_name) else {
            continue;
        };
        if text.as_bytes().get(open) != Some(&b'{') {
            continue;
        }
        if let Some(close) = matching_close_brace(text, open) {
            mods.push(InlineModuleScope { open, close, name });
        }
    }
    mods.sort_by_key(|module| module.open);
    mods
}

pub fn inline_scope_at(text: &str, at: usize) -> (Vec<String>, usize) {
    let modules: Vec<_> = inline_module_scopes(text)
        .into_iter()
        .filter(|module| module.open < at && at < module.close)
        .collect();
    let insertion = modules.last().map_or(0, |module| module.open);
    (
        modules.into_iter().map(|module| module.name).collect(),
        insertion,
    )
}

pub fn relative_import_path(
    caller_modules: &[String],
    trait_modules: &[String],
    trait_name: &str,
) -> Option<String> {
    let common = caller_modules
        .iter()
        .zip(trait_modules)
        .take_while(|(caller, target)| caller == target)
        .count();
    if common == caller_modules.len() && common == trait_modules.len() {
        return None;
    }
    let mut path = vec!["super".to_string(); caller_modules.len() - common];
    path.extend(trait_modules[common..].iter().cloned());
    path.push(trait_name.to_string());
    Some(path.join("::"))
}

pub fn add_import_at_inline_scope(text: &str, scope_open: usize, use_line: &str) -> String {
    if scope_open == 0 {
        return crate::move_item::add_import(text, use_line);
    }
    let Some(close) = matching_close_brace(text, scope_open) else {
        return text.to_string();
    };
    let body = &text[scope_open + 1..close];
    if body.contains(use_line) {
        return text.to_string();
    }
    let indent = body
        .lines()
        .find(|line| !line.trim().is_empty())
        .map_or("    ", |line| &line[..line.len() - line.trim_start().len()]);
    let updated = crate::move_item::add_import(body, &format!("{indent}{use_line}"));
    let mut out = String::with_capacity(text.len() + updated.len() - body.len());
    out.push_str(&text[..scope_open + 1]);
    out.push_str(&updated);
    out.push_str(&text[close..]);
    out
}

fn enclosing_inline_modules(text: &str, impl_at: usize) -> Vec<String> {
    inline_scope_at(text, impl_at).0
}

fn combine_scopes(a: Scope, b: Scope, inline_mods: &[String]) -> Scope {
    match (a, b) {
        (Scope::Pub, _) | (_, Scope::Pub) => Scope::Pub,
        (Scope::Private, other) | (other, Scope::Private) => other,
        (Scope::Crate, _) | (_, Scope::Crate) => Scope::Crate,
        (Scope::Super(x), Scope::Super(y)) => Scope::Super(x.max(y)),
        (Scope::CratePath(x), Scope::CratePath(y)) => common_prefix_scope(&x, &y),
        (Scope::Super(k), Scope::CratePath(segs)) | (Scope::CratePath(segs), Scope::Super(k)) => {
            if !inline_mods.is_empty() && segs.first() == inline_mods.first() {
                if k >= inline_mods.len() {
                    Scope::Crate
                } else {
                    let resolved = &inline_mods[..inline_mods.len() - k];
                    common_prefix_scope(resolved, &segs)
                }
            } else {
                Scope::Crate
            }
        }
    }
}

pub fn widest_visibility(text: &str, impl_at: usize, visibilities: &[&str]) -> String {
    let inline_mods = enclosing_inline_modules(text, impl_at);
    let mut widest = Scope::Private;
    for vis in visibilities {
        widest = combine_scopes(widest, parse_scope(vis), &inline_mods);
    }
    match widest {
        Scope::Private => String::new(),
        Scope::Super(1) => "pub(super)".to_string(),
        Scope::Super(k) => format!("pub(in {})", vec!["super"; k].join("::")),
        Scope::CratePath(segs) if segs.is_empty() => "pub(crate)".to_string(),
        Scope::CratePath(segs) => format!("pub(in crate::{})", segs.join("::")),
        Scope::Crate => "pub(crate)".to_string(),
        Scope::Pub => "pub".to_string(),
    }
}
