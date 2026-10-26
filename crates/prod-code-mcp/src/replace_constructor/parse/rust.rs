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
use anyhow::{Context, Result};
use ra_ap_syntax::{
    AstNode, Edition, SourceFile, ast,
    ast::{HasGenericParams, HasName, HasVisibility},
};

/// Parses Rust struct field declarations inside the struct body `{ ... }`.
pub fn parse_rust_struct_fields(inner: &str) -> Result<Vec<FieldDecl>> {
    Ok(parse_rust_struct_decl(&format!("struct __Fields {{ {inner} }}"), "__Fields")?.fields)
}

pub fn parse_rust_struct_decl(text: &str, type_name: &str) -> Result<StructDecl> {
    parse_with_edition(text, type_name, Edition::Edition2021)
}

pub(crate) fn parse_with_edition(
    text: &str,
    type_name: &str,
    edition: Edition,
) -> Result<StructDecl> {
    let parsed = SourceFile::parse(text, edition);
    anyhow::ensure!(
        parsed.errors().is_empty(),
        "cannot parse Rust constructor source: {}",
        parsed
            .errors()
            .first()
            .map(ToString::to_string)
            .unwrap_or_default()
    );
    let item = parsed
        .tree()
        .syntax()
        .descendants()
        .filter_map(ast::Struct::cast)
        .find(|item| item.name().is_some_and(|n| n.text() == type_name))
        .with_context(|| format!("cannot find declaration of struct `{type_name}` in file"))?;
    let Some(ast::FieldList::RecordFieldList(list)) = item.field_list() else {
        anyhow::bail!("constructor replacement requires a Rust record struct");
    };
    let fields = list
        .fields()
        .map(|field| {
            Ok(FieldDecl {
                name: field
                    .name()
                    .context("Rust field name is missing")?
                    .text()
                    .to_string(),
                ty: field
                    .ty()
                    .context("Rust field type is missing")?
                    .syntax()
                    .text()
                    .to_string(),
                vis: field
                    .visibility()
                    .map(|v| v.syntax().text().to_string())
                    .unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let name = item.name().context("Rust struct name is missing")?;
    let at = usize::from(name.syntax().text_range().start());
    let (line, col) = crate::signature::position_at(text, at)?;
    let generics = [
        item.generic_param_list()
            .map(|g| g.syntax().text().to_string()),
        item.where_clause().map(|w| w.syntax().text().to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    Ok(StructDecl {
        name: type_name.into(),
        language: "rust".into(),
        fields,
        generics: (!generics.is_empty()).then_some(generics),
        is_pub: item.visibility().is_some(),
        decl_start: usize::from(
            item.struct_token()
                .context("Rust struct keyword is missing")?
                .text_range()
                .start(),
        ),
        decl_end: usize::from(item.syntax().text_range().end()),
        line,
        col,
    })
}
