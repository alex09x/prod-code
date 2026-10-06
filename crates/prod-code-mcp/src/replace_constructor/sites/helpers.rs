/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::parse::is_ident;
use super::super::types::{InstantiationSite, StructDecl};
use anyhow::Result;

/// Rewrites a single instantiation site based on mode and language.
pub(crate) fn is_pure_reorder_value(value: &str) -> bool {
    let value = value.trim();
    if matches!(
        value,
        "true" | "false" | "nil" | "None" | "null" | "nullptr"
    ) {
        return true;
    }
    if value.chars().all(is_ident)
        || value
            .split("::")
            .all(|part| !part.is_empty() && part.chars().all(is_ident))
    {
        return true;
    }
    if matches!(value.chars().next(), Some('\'' | '"' | '`'))
        && value.ends_with(value.chars().next().unwrap())
        && !value.contains("${")
    {
        return true;
    }
    if let Some(prefix) = value
        .strip_suffix(".into()")
        .or_else(|| value.strip_suffix(".to_string()"))
        .or_else(|| value.strip_suffix(".to_owned()"))
        .or_else(|| value.strip_suffix(".clone()"))
    {
        if is_pure_reorder_value(prefix) {
            return true;
        }
    }
    value.trim_start_matches(['+', '-']).chars().all(|c| {
        c.is_ascii_digit() || matches!(c, '.' | '_' | 'x' | 'X' | 'u' | 'U' | 'i' | 'I' | 'f' | 'F')
    }) && value.chars().any(|c| c.is_ascii_digit())
}

pub(crate) fn go_zero_value(ty: &str) -> String {
    let ty = ty.trim();
    match ty {
        "bool" => "false".into(),
        "string" => "\"\"".into(),
        "byte" | "rune" | "int" | "int8" | "int16" | "int32" | "int64" | "uint" | "uint8"
        | "uint16" | "uint32" | "uint64" | "uintptr" | "float32" | "float64" | "complex64"
        | "complex128" => "0".into(),
        _ if ty.starts_with('*')
            || ty.starts_with("[]")
            || ty.starts_with("map[")
            || ty.starts_with("chan ")
            || ty.starts_with("func(")
            || ty.starts_with("interface{")
            || ty.starts_with("interface {")
            || ty == "any"
            || ty == "error" =>
        {
            "nil".into()
        }
        _ => format!("{ty}{{}}"),
    }
}

pub(crate) fn go_literal_values(
    site: &InstantiationSite,
    decl: &StructDecl,
) -> Result<Vec<Option<String>>> {
    let positional = site
        .field_values
        .keys()
        .any(|field| field.starts_with("__positional_"));
    if positional {
        anyhow::ensure!(
            site.field_values
                .keys()
                .all(|field| field.starts_with("__positional_")),
            "Go literals cannot mix keyed and positional elements"
        );
        anyhow::ensure!(
            site.field_values.len() <= decl.fields.len(),
            "Go positional literal has more values than declared fields"
        );
        return Ok(decl
            .fields
            .iter()
            .enumerate()
            .map(|(index, _)| {
                site.field_values
                    .get(&format!("__positional_{index}"))
                    .cloned()
            })
            .collect());
    }
    for field in site.field_values.keys() {
        anyhow::ensure!(
            decl.fields.iter().any(|declared| declared.name == *field),
            "Go literal refers to unknown field `{field}`"
        );
    }
    Ok(decl
        .fields
        .iter()
        .map(|field| site.field_values.get(&field.name).cloned())
        .collect())
}
