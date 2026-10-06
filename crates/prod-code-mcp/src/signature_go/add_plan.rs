/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::Param;
use crate::signature_go::syntax::{go_identifier, primitive_type, scalar_literal};
use crate::signature_go::text::{comments, is_ident_byte, skip_opaque, strip_comments};
use crate::signature_go::types::{Addition, GoParam, TextEdit, refusal};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};

/// The only addition shape supported in this increment: every old parameter is retained once in
/// its original order, with one or more new parameters inserted between them.
pub(crate) fn addition_plan(declared: &[GoParam], request: &[Param]) -> Result<Vec<Addition>> {
    let mut old_at = 0usize;
    let mut additions = Vec::new();
    let mut new_names = BTreeSet::new();
    for item in request {
        match item {
            Param::Keep(name) => {
                let expected = declared.get(old_at).map(|p| p.name.as_str());
                anyhow::ensure!(
                    expected == Some(name.as_str()),
                    "{}",
                    refusal(format!(
                        "an addition must retain every old parameter exactly once in its declared \
                         order; expected {} next, got `{name}`",
                        expected.map_or("no more old parameters".to_string(), |n| format!("`{n}`"))
                    ))
                );
                old_at += 1;
            }
            Param::Add { name, ty, value } => {
                anyhow::ensure!(
                    go_identifier(name),
                    "{}",
                    refusal(format!("`{name}` is not an ordinary Go parameter name"))
                );
                anyhow::ensure!(
                    !declared.iter().any(|p| p.name == *name) && new_names.insert(name.clone()),
                    "{}",
                    refusal(format!(
                        "the added parameter `{name}` duplicates another parameter"
                    ))
                );
                let ty = ty.trim();
                anyhow::ensure!(
                    primitive_type(ty),
                    "{}",
                    refusal(format!(
                        "the type of added `{name}` must be an ordinary primitive spelling, not \
                         `{ty}`"
                    ))
                );
                let value = value.trim();
                anyhow::ensure!(
                    scalar_literal(value),
                    "{}",
                    refusal(format!(
                        "the argument for added `{name}` must be one numeric, string or rune \
                         literal, not `{value}`"
                    ))
                );
                additions.push(Addition {
                    boundary: old_at,
                    name: name.clone(),
                    ty: ty.to_string(),
                    value: value.to_string(),
                });
            }
        }
    }
    anyhow::ensure!(
        old_at == declared.len(),
        "{}",
        refusal(format!(
            "an addition must retain every old parameter exactly once in its declared order; \
             {} would be removed",
            declared[old_at..]
                .iter()
                .map(|p| format!("`{}`", p.name))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    );
    anyhow::ensure!(!additions.is_empty(), "there is no parameter to add");
    Ok(additions)
}

pub(crate) fn addition_groups(additions: &[Addition]) -> BTreeMap<usize, Vec<&Addition>> {
    let mut groups: BTreeMap<usize, Vec<&Addition>> = BTreeMap::new();
    for addition in additions {
        groups.entry(addition.boundary).or_default().push(addition);
    }
    groups
}

pub(crate) fn requested_go_params(declared: &[GoParam], additions: &[Addition]) -> Vec<GoParam> {
    let groups = addition_groups(additions);
    let mut out = Vec::with_capacity(declared.len() + additions.len());
    for boundary in 0..=declared.len() {
        if let Some(group) = groups.get(&boundary) {
            out.extend(group.iter().map(|added| GoParam {
                name: added.name.clone(),
                ty: added.ty.clone(),
            }));
        }
        if let Some(old) = declared.get(boundary) {
            out.push(old.clone());
        }
    }
    out
}

pub(crate) fn requested_arguments(old: &[String], request: &[Param]) -> Vec<String> {
    let mut old_at = 0usize;
    let mut out = Vec::with_capacity(request.len());
    for item in request {
        match item {
            Param::Keep(_) => {
                out.push(old[old_at].clone());
                old_at += 1;
            }
            Param::Add { value, .. } => out.push(value.trim().to_string()),
        }
    }
    out
}

/// Zero-length edits that insert each group at its boundary. Every original byte remains where it
/// was; an existing comma becomes the separator before a middle or trailing insertion.
pub(crate) fn insertion_edits(
    text: &str,
    open: usize,
    close: usize,
    arity: usize,
    groups: &BTreeMap<usize, Vec<&Addition>>,
    declaration: bool,
) -> std::result::Result<Vec<TextEdit>, String> {
    let commas = top_level_commas(text, open + 1, close);
    let trailing = commas
        .last()
        .copied()
        .filter(|comma| strip_comments(&text[comma + 1..close]).trim().is_empty());
    if arity == 0 {
        if !strip_comments(&text[open + 1..close]).trim().is_empty() {
            return Err("the empty list contains text that cannot be attributed".to_string());
        }
    } else if commas.len() + usize::from(trailing.is_none()) != arity {
        return Err("the list's comma structure does not match its arity".to_string());
    }
    let mut out = Vec::new();
    for (&boundary, group) in groups {
        if boundary > arity {
            return Err(format!(
                "insertion boundary {boundary} is past arity {arity}"
            ));
        }
        if declaration && boundary > 0 && boundary < arity {
            let start = if boundary == 1 {
                open + 1
            } else {
                commas[boundary - 2] + 1
            };
            let end = commas[boundary - 1];
            if !parameter_piece_has_type(&text[start..end]) {
                return Err(format!(
                    "inserting after grouped parameter {} would change that old parameter's type",
                    boundary
                ));
            }
        }
        let inserted = group
            .iter()
            .map(|added| {
                if declaration {
                    format!("{} {}", added.name, added.ty)
                } else {
                    added.value.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let (at, replacement) = if arity == 0 {
            (open + 1, inserted)
        } else if boundary == 0 {
            (open + 1, format!("{inserted}, "))
        } else if boundary < arity {
            (commas[boundary - 1] + 1, format!(" {inserted},"))
        } else if let Some(comma) = trailing {
            if !comments(&text[comma + 1..close]).is_empty() {
                return Err(
                    "a trailing comment makes the last insertion's meaning ambiguous".to_string(),
                );
            }
            (comma + 1, format!(" {inserted},"))
        } else {
            (close, format!(", {inserted}"))
        };
        out.push((at, at, replacement));
    }
    Ok(out)
}

pub(crate) fn top_level_commas(text: &str, from: usize, to: usize) -> Vec<usize> {
    let bytes = text.as_bytes();
    let (mut depth, mut at) = (0i32, from);
    let mut out = Vec::new();
    while at < to {
        if let Some(end) = skip_opaque(bytes, at) {
            at = end;
            continue;
        }
        match bytes[at] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => out.push(at),
            _ => {}
        }
        at += 1;
    }
    out
}

pub(crate) fn parameter_piece_has_type(piece: &str) -> bool {
    let piece = strip_comments(piece);
    let piece = piece.trim();
    let word_end = piece
        .bytes()
        .position(|byte| !is_ident_byte(byte))
        .unwrap_or(piece.len());
    let word = &piece[..word_end];
    let rest = piece[word_end..].trim();
    let keyword = matches!(word, "chan" | "func" | "map" | "struct" | "interface");
    !word.is_empty()
        && !keyword
        && piece[word_end..].starts_with(|c: char| c.is_whitespace())
        && !rest.is_empty()
}
