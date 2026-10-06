/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `Order` for `Order`, `Wrapper<T>` and `crate::m::Order`.
pub(crate) fn base_name(ty: &str) -> &str {
    let ty = ty.trim();
    let ty = ty.split('<').next().unwrap_or(ty);
    ty.rsplit("::").next().unwrap_or(ty).trim()
}

/// `order` for `Order`, `line_item` for `LineItem`.
pub fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The type a receiver stood for, as a parameter's type: `&self` → `&Order`, `&mut self` →
/// `&mut Order`, `self` → `Order`; and whether the binding was `mut`. `None` for a receiver with
/// an explicit type (`self: Box<Self>`).
pub fn receiver_as_type(receiver: &str, owner: &str) -> Option<(String, bool)> {
    let r = receiver.trim();
    if r.contains(':') {
        return None;
    }
    match r {
        "self" => Some((owner.to_string(), false)),
        "mut self" => Some((owner.to_string(), true)),
        _ => {
            let rest = r.strip_prefix('&')?.trim_start();
            let (lifetime, rest) = if rest.starts_with('\'') {
                let end = rest.find(char::is_whitespace)?;
                (format!("{} ", &rest[..end]), rest[end..].trim_start())
            } else {
                (String::new(), rest)
            };
            match rest {
                "self" => Some((format!("&{lifetime}{owner}"), false)),
                "mut self" => Some((format!("&{lifetime}mut {owner}"), false)),
                _ => None,
            }
        }
    }
}

/// `text` with every whole identifier in `map` replaced, in one pass, so `self` → `order` and
/// `tax` → `self` do not run into each other.
pub fn swap_names(text: &str, map: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    let mut last = 0;
    while let Some((i, c)) = chars.next() {
        if !is_ident(c) || text[..i].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let mut end = i + c.len_utf8();
        while let Some((j, d)) = chars.peek().copied() {
            if is_ident(d) {
                end = j + d.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let word = &text[i..end];
        // A field or method named like the word (`x.self_`, `x.tax`) is not the binding.
        let after_dot = text[..i].ends_with('.') && !text[..i].ends_with("..");
        if let Some((_, to)) = map.iter().find(|(from, _)| *from == word)
            && !after_dot
        {
            out.push_str(&text[last..i]);
            out.push_str(to);
            last = end;
        }
    }
    out.push_str(&text[last..]);
    out
}
