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

use crate::make_static::Language;

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The base name of a type: `Wrapper` for `Wrapper<T>` and for `crate::m::Wrapper`.
pub(crate) fn base_name(ty: &str) -> &str {
    let ty = ty.trim();
    let ty = ty.split('<').next().unwrap_or(ty);
    ty.rsplit("::").next().unwrap_or(ty).trim()
}

/// The receiver a first parameter becomes, when its type is the `impl`'s own type: `c: &mut
/// Counter` → `&mut self`, `c: &'a Self` → `&'a self`, `mut c: Counter` → `mut self`. The binding's
/// name comes back with it.
pub fn receiver_for(param: &str, owner: &str) -> Option<(String, String)> {
    let (pattern, ty) = param.split_once(':')?;
    let pattern = pattern.trim();
    let (binding_mut, name) = match pattern.strip_prefix("mut ") {
        Some(rest) => (true, rest.trim()),
        None => (false, pattern),
    };
    if name.is_empty() || !name.chars().all(is_ident) || name == "self" {
        return None;
    }
    let ty = ty.trim();
    let (reference, rest) = match ty.strip_prefix('&') {
        None => (String::new(), ty),
        Some(rest) => {
            let rest = rest.trim_start();
            let (lifetime, rest) = if rest.starts_with('\'') {
                let end = rest.find(char::is_whitespace)?;
                (format!("{} ", &rest[..end]), rest[end..].trim_start())
            } else {
                (String::new(), rest)
            };
            match rest.strip_prefix("mut ") {
                Some(after) => (format!("&{lifetime}mut "), after.trim_start()),
                None => (format!("&{lifetime}"), rest),
            }
        }
    };
    let is_own = rest == "Self" || base_name(rest) == base_name(owner);
    if !is_own {
        return None;
    }
    let receiver = if reference.is_empty() && binding_mut {
        "mut self".to_string()
    } else {
        format!("{reference}self")
    };
    Some((receiver, name.to_string()))
}

/// The receiver a first argument becomes: `&mut c` and `&c` lose the borrow, which method-call
/// syntax takes by itself, and anything but a path or a call chain is parenthesized.
pub fn receiver_of(argument: &str) -> String {
    let a = argument.trim();
    let a = a
        .strip_prefix("&mut ")
        .or_else(|| a.strip_prefix('&'))
        .map(str::trim_start)
        .unwrap_or(a);
    let mut depth = 0i32;
    let mut simple = !a.is_empty() && a.chars().next().is_some_and(is_ident);
    for c in a.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if depth > 0 => {}
            c if is_ident(c) || c == '.' || c == ':' => {}
            _ => simple = false,
        }
    }
    if simple {
        a.to_string()
    } else {
        format!("({a})")
    }
}

/// Where the path in front of a call's name begins: `Counter::` or `crate::m::Counter::` or `Self::`.
pub(crate) fn path_start(text: &str, name_at: usize) -> usize {
    let mut at = name_at;
    loop {
        let before = &text[..at];
        let Some(stripped) = before.strip_suffix("::") else {
            return at;
        };
        let segment = stripped
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_ident(*c))
            .last()
            .map_or(stripped.len(), |(i, _)| i);
        if segment == stripped.len() {
            return at;
        }
        at = segment;
    }
}

pub fn split_call_arguments(args_str: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth_p = 0i32;
    let mut depth_b = 0i32;
    let mut depth_c = 0i32;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;

    for c in args_str.chars() {
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                in_quote = None;
            }
            current.push(c);
            continue;
        }
        match c {
            '"' | '\'' | '`' => {
                in_quote = Some(c);
                current.push(c);
            }
            '(' => {
                depth_p += 1;
                current.push(c);
            }
            ')' => {
                depth_p -= 1;
                current.push(c);
            }
            '[' => {
                depth_b += 1;
                current.push(c);
            }
            ']' => {
                depth_b -= 1;
                current.push(c);
            }
            '{' => {
                depth_c += 1;
                current.push(c);
            }
            '}' => {
                depth_c -= 1;
                current.push(c);
            }
            ',' if depth_p == 0 && depth_b == 0 && depth_c == 0 => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        args.push(current.trim().to_string());
    }
    args
}

pub(crate) fn receiver_argument<'a>(argument: &'a str, language: Language) -> &'a str {
    let argument = argument.trim();
    let label_separator = match language {
        Language::Swift => Some(':'),
        Language::Python => Some('='),
        _ => None,
    };
    let Some(separator) = label_separator else {
        return argument;
    };
    let Some((label, value)) = argument.split_once(separator) else {
        return argument;
    };
    let label = label.trim();
    let mut chars = label.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(is_ident);
    if valid { value.trim() } else { argument }
}
