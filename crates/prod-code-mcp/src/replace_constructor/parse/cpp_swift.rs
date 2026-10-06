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

/// Parses C++ struct/class fields inside `{ ... }`.
pub fn parse_cpp_fields(inner: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    for line in inner.lines() {
        let trimmed = line.trim().trim_end_matches(';');
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.contains('(') {
            continue;
        }
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        if tokens.len() >= 2 {
            let name = tokens.last().unwrap().trim_matches(['*', '&']);
            let ty = tokens[..tokens.len() - 1].join(" ");
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

/// Parses Swift struct/class fields inside `{ ... }`.
pub fn parse_swift_fields(inner: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    for line in inner.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("var ") || trimmed.starts_with("let ") {
            let decl = &trimmed[4..];
            if let Some((n, t)) = decl.split_once(':') {
                let n = n.trim();
                let t = t.split('=').next().unwrap_or(t).trim();
                if is_ident_str(n) {
                    fields.push(FieldDecl {
                        name: n.to_string(),
                        ty: t.to_string(),
                        vis: String::new(),
                    });
                }
            }
        }
    }
    fields
}

pub fn parse_cpp_struct_decl(text: &str, type_name: &str, language: &str) -> Result<StructDecl> {
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
        if before.ends_with("struct") || before.ends_with("class") {
            let Some(open_rel) = text[at + type_name.len()..].find('{') else {
                continue;
            };
            let open = at + type_name.len() + open_rel;
            let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
                continue;
            };
            let fields = parse_cpp_fields(&text[open + 1..close]);
            let (line, col) = crate::signature::position_at(text, at)?;
            return Ok(StructDecl {
                name: type_name.to_string(),
                language: language.to_string(),
                fields,
                generics: None,
                is_pub: true,
                decl_start: at,
                decl_end: close + 1,
                line,
                col,
            });
        }
    }
    anyhow::bail!("cannot find declaration of struct/class `{type_name}` in C++ file")
}

pub fn parse_swift_struct_decl(text: &str, type_name: &str) -> Result<StructDecl> {
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
        if before.ends_with("struct") || before.ends_with("class") {
            let Some(open_rel) = text[at + type_name.len()..].find('{') else {
                continue;
            };
            let open = at + type_name.len() + open_rel;
            let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
                continue;
            };
            let fields = parse_swift_fields(&text[open + 1..close]);
            let (line, col) = crate::signature::position_at(text, at)?;
            return Ok(StructDecl {
                name: type_name.to_string(),
                language: "swift".to_string(),
                fields,
                generics: None,
                is_pub: before.contains("public") || before.contains("open"),
                decl_start: at,
                decl_end: close + 1,
                line,
                col,
            });
        }
    }
    anyhow::bail!("cannot find declaration of struct/class `{type_name}` in Swift file")
}
