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
use super::{is_ident, is_ident_str, split_balanced_commas};
use anyhow::Result;

/// Parses TypeScript / JavaScript class/interface fields.
pub fn parse_ts_fields(inner: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    if let Some(pos) = inner.find("constructor")
        && let Some(open) = inner[pos..].find('(')
        && let Some(close) = inner[pos + open..].find(')')
    {
        let params = &inner[pos + open + 1..pos + open + close];
        for p in split_balanced_commas(params) {
            let clean = p
                .trim_start_matches("public ")
                .trim_start_matches("private ")
                .trim_start_matches("protected ")
                .trim_start_matches("readonly ")
                .trim();
            if let Some((n, t)) = clean.split_once(':') {
                let name = n.trim().to_string();
                if is_ident_str(&name) && !fields.iter().any(|f: &FieldDecl| f.name == name) {
                    fields.push(FieldDecl {
                        name,
                        ty: t.trim().to_string(),
                        vis: String::new(),
                    });
                }
            }
        }
    }
    if !fields.is_empty() {
        return fields;
    }
    for line in inner.lines() {
        let trimmed = line.trim().trim_end_matches(';');
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with("this.")
            || trimmed.starts_with("return")
            || trimmed.contains('(')
        {
            continue;
        }
        if let Some((left, right)) = trimmed.split_once(':') {
            let left = left
                .trim_start_matches("public ")
                .trim_start_matches("private ")
                .trim_start_matches("protected ")
                .trim_start_matches("readonly ")
                .trim();
            if is_ident_str(left) && !fields.iter().any(|f: &FieldDecl| f.name == left) {
                fields.push(FieldDecl {
                    name: left.to_string(),
                    ty: right.trim().to_string(),
                    vis: String::new(),
                });
            }
        }
    }
    fields
}

pub fn parse_javascript_fields(body: &str) -> Vec<FieldDecl> {
    let mut fields: Vec<FieldDecl> = Vec::new();
    let mut brace_depth = 0i32;
    for line in body.lines() {
        let trimmed = line.trim();
        if brace_depth == 0 && !trimmed.is_empty() && !trimmed.starts_with("//") {
            let candidate = trimmed
                .split(['=', ';'])
                .next()
                .unwrap_or("")
                .trim_start()
                .trim_start_matches("public ")
                .trim_start_matches("private ")
                .trim_start_matches("protected ")
                .trim_start_matches("static ")
                .trim();
            if is_ident_str(candidate) && !fields.iter().any(|field| field.name == candidate) {
                fields.push(FieldDecl {
                    name: candidate.to_string(),
                    ty: String::new(),
                    vis: String::new(),
                });
            }
        }
        brace_depth += line.chars().filter(|ch| *ch == '{').count() as i32;
        brace_depth -= line.chars().filter(|ch| *ch == '}').count() as i32;
    }

    if let Some(constructor_at) = body.find("constructor") {
        let after_constructor = &body[constructor_at + "constructor".len()..];
        if let Some(params_rel) = after_constructor.find('(') {
            let params_open = constructor_at + "constructor".len() + params_rel;
            if let Some(params_close) = crate::parameter_object::matching_bracket(body, params_open)
            {
                let parameters = split_balanced_commas(&body[params_open + 1..params_close]);
                let after_params = &body[params_close + 1..];
                if let Some(body_open_rel) = after_params.find('{') {
                    let body_open = params_close + 1 + body_open_rel;
                    if let Some(body_close) =
                        crate::parameter_object::matching_bracket(body, body_open)
                    {
                        let constructor_body = &body[body_open + 1..body_close];
                        for parameter in parameters {
                            let name = parameter
                                .trim()
                                .trim_start_matches("...")
                                .split(['=', ':'])
                                .next()
                                .unwrap_or("")
                                .trim();
                            if !is_ident_str(name)
                                || !constructor_body.contains(&format!("this.{name}"))
                                || fields.iter().any(|field| field.name == name)
                            {
                                continue;
                            }
                            fields.push(FieldDecl {
                                name: name.to_string(),
                                ty: String::new(),
                                vis: String::new(),
                            });
                        }
                    }
                }
            }
        }
    }
    fields
}

pub fn parse_ts_struct_decl(text: &str, type_name: &str, language: &str) -> Result<StructDecl> {
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
        let is_target = before.ends_with("class") || before.ends_with("interface");
        if !is_target {
            continue;
        }
        let Some(open_rel) = text[at + type_name.len()..].find('{') else {
            continue;
        };
        let open = at + type_name.len() + open_rel;
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let body = &text[open + 1..close];
        let fields = if matches!(language, "javascript" | "javascriptreact") {
            parse_javascript_fields(body)
        } else {
            parse_ts_fields(body)
        };
        let (line, col) = crate::signature::position_at(text, at)?;
        return Ok(StructDecl {
            name: type_name.to_string(),
            language: language.to_string(),
            fields,
            generics: None,
            is_pub: before.contains("export"),
            decl_start: at,
            decl_end: close + 1,
            line,
            col,
        });
    }
    anyhow::bail!(
        "cannot find declaration of class/interface `{type_name}` in TypeScript/JavaScript file"
    )
}
