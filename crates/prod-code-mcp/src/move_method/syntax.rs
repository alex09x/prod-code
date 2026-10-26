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
    let mut i = 0;
    let mut last = 0;
    while i < text.len() {
        match opaque_token(text, i) {
            Ok(Some((end, _))) => {
                i = end;
                continue;
            }
            Err(()) => break, // An incomplete opaque token is never identifier text.
            Ok(None) => {}
        }
        let c = text[i..].chars().next().unwrap();
        if !is_ident(c) {
            i += c.len_utf8();
            continue;
        }
        let token = super::context::word_at(text, i).unwrap();
        let end = token.end;
        let word = token.name;
        // Nested owners have their own Self and receiver bindings. Closures, by contrast,
        // capture the outer receiver and must still be visited.
        if let Some(end) = token.owner_end(text) {
            i = end;
            continue;
        }
        // A field or method named like the word (`x.self_`, `x.tax`) is not the binding.
        let before = text[..i].trim_end();
        let after_dot = before.ends_with('.') && !before.ends_with("..");
        let module_self = word == "self" && text[end..].trim_start().starts_with("::");
        if let Some((_, to)) = map.iter().find(|(from, _)| *from == word)
            && !after_dot
            && !module_self
        {
            out.push_str(&text[last..i]);
            out.push_str(to);
            last = end;
        }
        i = end;
    }
    out.push_str(&text[last..]);
    out
}

/// receiver_for has already accepted a plain or mutable identifier pattern.
/// Its binding is the final token, never a substring of the `mut` modifier.
pub(super) fn binding_offset(pattern: &str, binding: &str) -> Option<usize> {
    let pattern = pattern.trim_end();
    let prefix = pattern.strip_suffix(binding)?;
    (!prefix.ends_with(is_ident)).then_some(prefix.len())
}

/// Consume raw byte/C literals from their prefix, before `br` becomes an identifier.
pub(super) fn opaque_token(text: &str, at: usize) -> Result<Option<(usize, bool)>, ()> {
    if matches!(text.as_bytes()[at], b'b' | b'c')
        && text.as_bytes().get(at + 1) == Some(&b'r')
        && let Some(token) = crate::signature::opaque_at(text, at + 1, 0)?
    {
        return Ok(Some(token));
    }
    crate::signature::opaque_at(text, at, 0)
}

/// A declaration-shaped macro input may expand as an owner or merely consume its
/// tokens as data. Without expansion identity neither keeping nor replacing Self is
/// justified. Reject that ambiguity before planning, including forced moves.
pub(crate) fn ensure_owner_context(text: &str) -> anyhow::Result<()> {
    let mut context = super::context::MacroContext::default();
    let mut i = 0;
    while i < text.len() {
        if let Some((end, comment)) = opaque_token(text, i)
            .map_err(|()| anyhow::anyhow!("incomplete token; cannot establish Self ownership"))?
        {
            if !comment {
                context.literal();
            }
            i = end;
            continue;
        }
        let c = text[i..].chars().next().unwrap();
        if let Some(word) = super::context::word_at(text, i) {
            i = word.end;
            context.word(word.name, word.raw);
            if let Some(end) = word.owner_end(text) {
                anyhow::ensure!(
                    !context.in_macro(),
                    "cannot establish Self ownership of an owner declaration inside macro tokens; nothing was planned"
                );
                // Independent nested owners are outside the moved owner's scope.
                i = end;
            }
            continue;
        }
        context.punctuation(c);
        i += c.len_utf8();
    }
    Ok(())
}

/// Only a complete owner declaration introduces another Self. A keyword in a macro
/// token tree (`size!(impl Self)`) must not consume the enclosing expression's remainder.
pub(super) fn nested_owner_end(text: &str, mut i: usize, keyword: &str) -> Option<usize> {
    let header = i;
    let mut delimiters = super::context::TypeDelimiters::default();
    let mut macros = super::context::MacroContext::default();
    while i < text.len() {
        if let Some((end, comment)) = opaque_token(text, i).ok()? {
            if !comment {
                macros.literal();
            }
            i = end;
            continue;
        }
        let c = text[i..].chars().next()?;
        if let Some(word) = super::context::word_at(text, i) {
            i = word.end;
            macros.word(word.name, word.raw);
            continue;
        }
        let in_macro = macros.in_macro();
        macros.punctuation(c);
        // Macro arguments are token trees, not Rust generic/type syntax.
        // Their comparisons, arrows and braces cannot delimit the owner.
        if in_macro || macros.in_macro() {
            i += c.len_utf8();
            continue;
        }
        match c {
            ';' if delimiters.top().is_none() => {
                // Only an out-of-line module can be an owner without a body.
                let name = text[header..i].trim();
                return (keyword == "mod"
                    && !matches!(name, "Self" | "self")
                    && name
                        .chars()
                        .next()
                        .is_some_and(|c| c == '_' || c.is_alphabetic())
                    && name.chars().all(is_ident))
                .then_some(i + 1);
            }
            // Header generics end here. Expressions in the body can contain comparisons
            // and shifts: their angle tokens have no bearing on the owner's brace span.
            '{' if delimiters.top().is_none() => return owner_body_end(text, i),
            // A braced const argument contains expressions, not generic delimiters.
            // Skip it as a balanced token tree so comparisons/shifts do not close
            // the surrounding owner header or leave an unmatched angle opener.
            '{' => {
                i = owner_body_end(text, i)?;
                macros.punctuation('}');
                continue;
            }
            '(' | '[' => delimiters.push(c, false),
            ';' if delimiters.top() == Some('[') => delimiters.start_length(),
            '<' if !delimiters.array_length() => delimiters.push(c, false),
            '<' | '>' if delimiters.array_length() => {}
            '>' if text[..i].ends_with('-') => {}
            ')' | ']' | '>' => {
                let open = match c {
                    ')' => '(',
                    ']' => '[',
                    _ => '<',
                };
                delimiters.close(open)?;
            }
            // Crossing the macro's closing delimiter is not an owner declaration.
            '}' => return None,
            _ => {}
        }
        i += c.len_utf8();
    }
    None
}

fn owner_body_end(text: &str, mut i: usize) -> Option<usize> {
    let mut depth = 0usize;
    while i < text.len() {
        if let Some((end, _)) = opaque_token(text, i).ok()? {
            i = end;
            continue;
        }
        let c = text[i..].chars().next()?;
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += c.len_utf8();
    }
    None
}

/// Strip a reference's lifetime and mutability without losing its complete referent type.
pub(crate) fn referent(ty: &str) -> Option<&str> {
    let Some(rest) = ty.trim().strip_prefix('&') else {
        return Some(ty.trim());
    };
    let mut rest = rest.trim_start();
    if rest.starts_with('\'') {
        let end = rest.find(char::is_whitespace)?;
        rest = rest[end..].trim_start();
    }
    Some(rest.strip_prefix("mut ").unwrap_or(rest).trim_start())
}
