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

#[cfg(test)]
mod tests;

/// Parses Python class fields from `def __init__` or dataclass annotations.
pub fn parse_python_fields(text: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    if let Some(init_pos) = text.find("def __init__")
        && let Some(open) = text[init_pos..].find('(')
        && let Some(close) = text[init_pos + open..].find(')')
    {
        let params = &text[init_pos + open + 1..init_pos + open + close];
        for p in split_balanced_commas(params) {
            let p = p.trim();
            if p == "self" || p.is_empty() {
                continue;
            }
            if let Some((n, t)) = p.split_once(':') {
                let n = n.trim().split('=').next().unwrap_or(n).trim();
                let t = t.trim().split('=').next().unwrap_or(t).trim();
                fields.push(FieldDecl {
                    name: n.to_string(),
                    ty: t.to_string(),
                    vis: String::new(),
                });
            } else {
                let n = p.split('=').next().unwrap_or(p).trim();
                fields.push(FieldDecl {
                    name: n.to_string(),
                    ty: "Any".to_string(),
                    vis: String::new(),
                });
            }
        }
    }
    if fields.is_empty() {
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("def ") || trimmed.starts_with('@') || trimmed.is_empty() {
                continue;
            }
            if let Some((n, t)) = trimmed.split_once(':') {
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

pub fn parse_python_struct_decl(text: &str, type_name: &str) -> Result<StructDecl> {
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
        if before.ends_with("class") {
            let class_start = text[..at].rfind("class").unwrap_or(at);
            let (line, col) = crate::signature::position_at(text, at)?;
            // The matched declaration owns the range, not an earlier name that
            // happens to contain this one (BaseConfig before Config, for example).
            let line_start = text[..class_start].rfind('\n').map_or(0, |i| i + 1);
            let class_indent = class_start - line_start;
            let header_end = text[at..].find('\n').map_or(text.len(), |i| at + i + 1);
            let mut end_offset = text.len();
            for l in text[header_end..].lines() {
                let trimmed = l.trim_start();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    let indent = l.len() - trimmed.len();
                    if indent <= class_indent {
                        let l_ptr = l.as_ptr() as usize;
                        let text_ptr = text.as_ptr() as usize;
                        end_offset = l_ptr - text_ptr;
                        break;
                    }
                }
            }
            let body = text.get(header_end..end_offset).ok_or_else(|| {
                anyhow::anyhow!("invalid Python class bounds; nothing was written")
            })?;
            let fields = super::parameters::constructor_parameters(body, true)?
                .unwrap_or_else(|| parse_annotation_fields(body));
            return Ok(StructDecl {
                name: type_name.to_string(),
                language: "python".to_string(),
                fields,
                generics: None,
                is_pub: !type_name.starts_with('_'),
                decl_start: class_start,
                decl_end: end_offset,
                line,
                col,
            });
        }
    }
    anyhow::bail!("cannot find declaration of class `{type_name}` in Python file")
}

/// Only direct annotation members belong to a dataclass constructor. Keep the
/// initializer at the original declaration; omission must reach the dataclass.
fn parse_annotation_fields(body: &str) -> Vec<FieldDecl> {
    let mask =
        crate::caller_migration::lexical_code_mask(body, crate::parameter_object::Language::Python);
    let member_indent = super::parameters::python_member_indent(body, &mask);
    body.lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            let at = line.as_ptr() as usize - body.as_ptr() as usize + indent;
            if Some(indent) != member_indent || !mask.get(at).copied().unwrap_or(false) {
                return None;
            }
            let (name, annotation) = trimmed.split_once(':')?;
            if !is_ident_str(name.trim()) {
                return None;
            }
            let (ty, default) = crate::parameter_object::split_default(
                annotation,
                crate::parameter_object::Language::Python,
            );
            Some(FieldDecl {
                name: name.trim().into(),
                ty: ty.trim().into(),
                vis: if default.is_some() { "default" } else { "" }.into(),
            })
        })
        .collect()
}
