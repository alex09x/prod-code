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
use super::{is_ident, is_ident_str};
use anyhow::Result;

/// Parses Go struct field declarations inside `{ ... }`.
pub fn parse_go_struct_fields(inner: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    for line in inner.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        let clean = if let Some((code, _)) = trimmed.split_once("//") {
            code.trim()
        } else {
            trimmed
        };
        let clean = if let Some(tag_start) = clean.find('`') {
            clean[..tag_start].trim()
        } else {
            clean
        };
        let tokens: Vec<&str> = clean.split_whitespace().collect();
        if tokens.len() >= 2 {
            let name = tokens[0];
            let ty = tokens[1..].join(" ");
            if is_ident_str(name) {
                fields.push(FieldDecl {
                    name: name.to_string(),
                    ty,
                    vis: String::new(),
                });
            }
        }
    }
    fields
}

pub fn parse_go_struct_decl(text: &str, type_name: &str) -> Result<StructDecl> {
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
        let after = &text[at + type_name.len()..];
        if let Some(open) = after.find('{') {
            let head = &after[..open];
            if head.contains("struct") {
                let open_idx = at + type_name.len() + open;
                if let Some(close) = crate::parameter_object::matching_bracket(text, open_idx) {
                    let fields = parse_go_struct_fields(&text[open_idx + 1..close]);
                    let (line, col) = crate::signature::position_at(text, at)?;
                    let is_pub = type_name.chars().next().is_some_and(|c| c.is_uppercase());
                    return Ok(StructDecl {
                        name: type_name.to_string(),
                        language: "go".to_string(),
                        fields,
                        generics: None,
                        is_pub,
                        decl_start: at,
                        decl_end: close + 1,
                        line,
                        col,
                    });
                }
            }
        }
    }
    anyhow::bail!("cannot find declaration of struct `{type_name}` in Go file")
}
