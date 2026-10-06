/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::enclosing_open_brace;

pub(crate) fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// What a boolean name at a declaration is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueKind {
    /// A field of the struct whose header (`… struct Name …`) starts at `header`.
    Field { header: usize },
    /// A `let` binding; `annotated` when it is written `name: bool`.
    Local { annotated: bool },
}

/// The kind of declaration the name starting at `start` is, when it is one this can invert.
pub fn value_kind(text: &str, start: usize, name: &str) -> Option<ValueKind> {
    let after = text[start + name.len()..].trim_start();
    let before = text[..start].trim_end();
    let before_let = before
        .strip_suffix("mut")
        .map(str::trim_end)
        .unwrap_or(before);
    if before_let.ends_with("let")
        && !before_let[..before_let.len() - 3]
            .chars()
            .next_back()
            .is_some_and(is_ident)
    {
        let annotated = after.strip_prefix(':').is_some_and(|t| {
            t.trim_start().starts_with("bool") && !t.trim_start()[4..].starts_with(is_ident)
        });
        return Some(ValueKind::Local { annotated });
    }
    // `name: bool` inside the braces of a `struct`.
    let ty = after.strip_prefix(':').filter(|t| !t.starts_with(':'))?;
    let ty_end = ty.find([',', '}', '\n']).unwrap_or(ty.len());
    if ty[..ty_end].trim() != "bool" {
        return None;
    }
    let open = enclosing_open_brace(text, start)?;
    let header_start = text[..open].rfind(['\n', ';', '}']).map_or(0, |i| i + 1);
    let header = &text[header_start..open];
    header
        .split(|c: char| !is_ident(c))
        .any(|word| word == "struct")
        .then_some(ValueKind::Field {
            header: header_start,
        })
}
