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

use super::super::common::{is_ident, split_top};
use super::types::{Field, StructDecl};

/// The struct whose `struct` keyword is on the line of `at`.
pub fn parse_struct(text: &str, at: usize) -> Result<StructDecl> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let kw = text[line_start..line_end]
        .find("struct ")
        .map(|i| line_start + i)
        .context("no `struct` on this line")?;
    let vis = text[line_start..kw].trim().to_string();
    let name: String = text[kw + 7..]
        .trim_start()
        .chars()
        .take_while(|c| is_ident(*c))
        .collect();
    anyhow::ensure!(!name.is_empty(), "the struct has no name");
    let after_name = kw + 7 + text[kw + 7..].find(&name).unwrap_or(0) + name.len();
    let rest = text[after_name..].trim_start();
    anyhow::ensure!(
        rest.starts_with('{'),
        "`{name}` is not a plain struct with named fields (generic and tuple structs are not supported)"
    );
    let open = after_name + (text[after_name..].len() - rest.len());
    let close = crate::parameter_object::matching_bracket(text, open)
        .context("the struct is not closed")?;
    // The attributes and doc comments above belong to the declaration.
    let mut start = line_start;
    let mut derive = None;
    loop {
        let above = text[..start].trim_end_matches('\n');
        let prev_start = above.rfind('\n').map_or(0, |i| i + 1);
        let prev = above[prev_start..].trim();
        if start == 0 || !(prev.starts_with("#[") || prev.starts_with("///")) {
            break;
        }
        if prev.starts_with("#[derive(") {
            derive = Some(prev.to_string());
        }
        start = prev_start;
    }
    let body = &text[open + 1..close];
    let mut fields = Vec::new();
    for (s, e) in split_top(body, ',') {
        let chunk = body[s..e].trim();
        let decl = crate::extract_trait::declaration(chunk).1;
        let (fvis, bare) = crate::extract_trait::visibility(decl);
        let Some((fname, _)) = bare.split_once(':') else {
            continue;
        };
        fields.push(Field {
            name: fname.trim().to_string(),
            vis: fvis.to_string(),
            text: chunk.to_string(),
        });
    }
    Ok(StructDecl {
        name,
        vis,
        start,
        open,
        close,
        derive,
        fields,
    })
}

/// The inherent `impl Name` blocks of `text`, as (start of `impl`, open, close).
pub fn impl_blocks(text: &str, name: &str) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices("impl ") {
        if text[..i].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let Some(open) = text[i..].find('{').map(|o| i + o) else {
            continue;
        };
        let header = text[i + 5..open].trim();
        if header == name
            && let Some(close) = crate::parameter_object::matching_bracket(text, open)
        {
            out.push((i, open, close));
        }
    }
    out
}

/// The parameter names of a signature, `self` left out: `(&self, a: u32, mut b: T)` gives `a, b`.
pub fn argument_names(sig: &str) -> Option<Vec<String>> {
    let open = sig.find('(')?;
    let close = crate::parameter_object::matching_bracket(sig, open)?;
    let mut out = Vec::new();
    for (s, e) in split_top(&sig[open + 1..close], ',') {
        let p = sig[open + 1 + s..open + 1 + e].trim();
        if p.is_empty() || p.ends_with("self") {
            continue;
        }
        let pat = p.split_once(':')?.0.trim();
        let pat = pat.strip_prefix("mut ").unwrap_or(pat);
        if !pat.chars().all(is_ident) {
            return None;
        }
        out.push(pat.to_string());
    }
    Some(out)
}
