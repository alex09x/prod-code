/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::{FieldDecl, StructDecl};
use super::{extract_generics, is_ident, is_ident_str, split_balanced_commas};
use anyhow::{Context, Result};

/// Parses Rust struct field declarations inside the struct body `{ ... }`.
pub fn parse_rust_struct_fields(inner: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    let chunks = split_balanced_commas(inner);
    for raw in chunks {
        let lines: Vec<&str> = raw
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with("#["))
            .collect();
        let cleaned = lines.join(" ");
        let Some((left, right)) = cleaned.split_once(':') else {
            continue;
        };
        let ty = right.trim().to_string();
        let mut words = left.split_whitespace();
        let mut vis = String::new();
        let mut name = String::new();
        for word in words.by_ref() {
            if word.starts_with("pub") {
                vis = word.to_string();
            } else if is_ident_str(word) {
                name = word.to_string();
            }
        }
        if !name.is_empty() && !ty.is_empty() {
            fields.push(FieldDecl { name, ty, vis });
        }
    }
    fields
}

pub fn parse_rust_struct_decl(text: &str, type_name: &str) -> Result<StructDecl> {
    let mut candidate = None;
    for (at, _) in text.match_indices(type_name) {
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + type_name.len()..]
            .chars()
            .next()
            .is_some_and(is_ident)
        {
            continue;
        }
        let before = text[..at].trim_end();
        let is_struct = before.ends_with("struct")
            || before.ends_with("struct ")
            || before.contains("struct ")
                && before[before.rfind("struct ").unwrap()..]
                    .chars()
                    .all(|c| c.is_whitespace() || is_ident(c) || c == '(' || c == ')');
        if !is_struct {
            continue;
        }
        let struct_kw = before.rfind("struct").unwrap_or(at);
        let line_start = text[..struct_kw].rfind('\n').map_or(0, |i| i + 1);
        let is_pub = text[line_start..struct_kw].contains("pub");
        let generics = extract_generics(text, at + type_name.len());
        let open_from = generics
            .as_ref()
            .map_or(at + type_name.len(), |(_, end)| *end);
        let Some(open_rel) = text[open_from..].find('{') else {
            continue;
        };
        let open = open_from + open_rel;
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let fields = parse_rust_struct_fields(&text[open + 1..close]);
        let (line, col) = crate::signature::position_at(text, at)?;
        candidate = Some(StructDecl {
            name: type_name.to_string(),
            language: "rust".to_string(),
            fields,
            generics: generics.map(|(g, _)| g),
            is_pub,
            decl_start: struct_kw,
            decl_end: close + 1,
            line,
            col,
        });
        break;
    }
    candidate.with_context(|| format!("cannot find declaration of struct `{type_name}` in file"))
}
