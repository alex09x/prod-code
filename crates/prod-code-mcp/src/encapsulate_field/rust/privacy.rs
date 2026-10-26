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
use std::ops::Range;

/// The innermost inline module containing the declaration. Descendant modules lie inside
/// this range too. Strings, raw strings and nested comments cannot introduce fake modules.
pub(super) fn scope(text: &str, declaration: usize) -> Result<Range<usize>> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if let Some((end, _)) = crate::signature::call_sites::opaque_at(text, i, 0)
            .ok()
            .context("cannot determine Rust privacy scope: unterminated literal or comment")?
        {
            i = end;
            continue;
        }
        let ch = text[i..].chars().next().unwrap();
        if ch.is_whitespace() {
            i += ch.len_utf8();
            continue;
        }
        let start = i;
        i += ch.len_utf8();
        // A raw module identifier is one token, just like an ordinary identifier.
        if ch == 'r'
            && text[i..].starts_with('#')
            && text[i + 1..].starts_with(super::super::case::is_ident)
        {
            i += 1;
        }
        if super::super::case::is_ident(ch) {
            while i < text.len() && text[i..].starts_with(super::super::case::is_ident) {
                i += text[i..].chars().next().unwrap().len_utf8();
            }
        }
        tokens.push((start, &text[start..i]));
    }
    let mut scope = 0..text.len();
    let mut stack = Vec::new();
    for (n, (at, token)) in tokens.iter().enumerate() {
        match *token {
            "{" => stack.push((*at, n >= 2 && tokens[n - 2].1 == "mod")),
            "}" => {
                let (open, module) = stack.pop().context("unbalanced Rust privacy scope")?;
                if module && open < declaration && declaration < *at && open >= scope.start {
                    scope = open..*at;
                }
            }
            _ => {}
        }
    }
    Ok(scope)
}

/// An equally named type in another inline module does not own these accessors.
pub(super) fn implementations(
    text: &str,
    owner: &str,
    privacy: &Range<usize>,
) -> Result<Vec<(usize, usize)>> {
    let mut found = Vec::new();
    let mut from = privacy.start;
    while let Some(relative) = super::analysis::inherent_impl(&text[from..privacy.end], owner) {
        let open = from + relative;
        let close = crate::parameter_object::matching_bracket(text, open)
            .context("inherent impl does not close")?;
        if scope(text, open)? == *privacy {
            found.push((open, close));
        }
        from = close + 1;
    }
    Ok(found)
}
