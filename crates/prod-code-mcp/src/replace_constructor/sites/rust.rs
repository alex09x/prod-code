/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::InstantiationSite;
use ra_ap_syntax::{AstNode, Edition, SourceFile, ast};
use std::collections::BTreeMap;

/// Cargo projects keep their declared edition (and manifest errors). Standalone
/// files use the same 2021 fallback as the manifest-free discovery API below.
pub(crate) fn consumer_edition(path: &std::path::Path) -> anyhow::Result<Edition> {
    let mut edition = 2021;
    for directory in path.ancestors().skip(1) {
        if directory.join("Cargo.toml").try_exists()? {
            edition = crate::parameter_object::rust_edition(path).map_err(anyhow::Error::msg)?;
            break;
        }
    }
    Ok(match edition {
        2015 => Edition::Edition2015,
        2018 => Edition::Edition2018,
        2021 => Edition::Edition2021,
        2024 => Edition::Edition2024,
        _ => anyhow::bail!("unsupported Rust consumer edition {edition}"),
    })
}

/// Finds Rust struct expressions in source without a manifest (edition 2021).
/// Crate-qualified paths require file context; this API cannot assume a crate root.
/// Workspace refactoring uses the consumer's manifest edition and crate identity.
pub fn find_rust_instantiations(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
) -> (Vec<InstantiationSite>, Vec<String>) {
    find_with_edition(
        text,
        type_name,
        decl_start,
        decl_end,
        Edition::Edition2021,
        &[],
        &super::rust_crate::CrateRoot::Unknown,
    )
}

pub(crate) fn find_in_file(
    path: &std::path::Path,
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
    references: &[(u32, u32)],
) -> anyhow::Result<(Vec<InstantiationSite>, Vec<String>)> {
    let edition = consumer_edition(path)?;
    let crate_root = super::rust_crate::CrateRoot::for_file(path, edition)?;
    Ok(find_with_edition(
        text,
        type_name,
        decl_start,
        decl_end,
        edition,
        references,
        &crate_root,
    ))
}

fn find_with_edition(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
    edition: Edition,
    reference_positions: &[(u32, u32)],
    crate_root: &super::rust_crate::CrateRoot,
) -> (Vec<InstantiationSite>, Vec<String>) {
    let parse = SourceFile::parse(text, edition);
    if !parse.errors().is_empty() {
        return (
            Vec::new(),
            vec![format!(
                "cannot parse Rust constructor source: {}",
                parse.errors()[0]
            )],
        );
    }
    let root = parse.tree();
    let selected = super::rust_identity::selected_declaration(root.syntax(), decl_start, decl_end);
    let mut sites = Vec::new();
    let mut blocked = super::rust_macros::blocked_references(
        root.syntax(),
        text,
        type_name,
        selected.as_ref(),
        reference_positions,
        crate_root,
    );
    for literal in root
        .syntax()
        .descendants()
        .filter_map(ast::RecordExpr::cast)
    {
        let Some(path) = literal.path() else { continue };
        let Some(segment) = path.segment() else {
            continue;
        };
        let Some(name) = segment.name_ref() else {
            continue;
        };
        let name = name.text();
        if name != type_name && name != "Self" {
            continue;
        }
        let identity = super::rust_identity::is_selected(
            &path,
            selected.as_ref(),
            type_name,
            crate_root,
            |owner| {
                owner
                    .segment()
                    .and_then(|s| s.name_ref())
                    .is_some_and(|name| {
                        crate::signature::line_col_at(
                            text,
                            usize::from(name.syntax().text_range().start()),
                        )
                        .is_some_and(|position| reference_positions.contains(&position))
                    })
            },
        );
        match identity {
            Ok(true) => {}
            Ok(false) => continue,
            Err(()) => {
                let offset = usize::from(path.syntax().text_range().start());
                let (line, col) = crate::signature::position_at(text, offset).unwrap_or((0, 0));
                blocked.push(format!(
                    "{line}:{col} cannot resolve Rust constructor owner through ambiguous or unavailable imports or crate identity"
                ));
                continue;
            }
        }
        let range = literal.syntax().text_range();
        let start = usize::from(range.start());
        if start >= decl_start && start < decl_end {
            continue;
        }
        let Some(fields) = literal.record_expr_field_list() else {
            continue;
        };
        if fields.dotdot_token().is_some() {
            let (l, c) = crate::signature::position_at(text, start).unwrap_or((0, 0));
            blocked.push(format!(
                "{l}:{c} instantiation uses struct update syntax `..` which cannot be mapped to positional constructor arguments"
            ));
            continue;
        }
        let mut field_values = BTreeMap::new();
        let mut field_order = Vec::new();
        for field in fields.fields() {
            // Use the expression grammar, never the generic-declaration comma splitter.
            // Slice original byte ranges so comments and literal spelling survive.
            let Some(key) = field.field_name() else {
                continue;
            };
            let key = key.text().to_string();
            let value = if let Some(colon) = field.colon_token() {
                let from = usize::from(colon.text_range().end());
                let to = usize::from(field.syntax().text_range().end());
                text[from..to].trim().to_string()
            } else {
                key.clone()
            };
            field_order.push(key.clone());
            field_values.insert(key, value);
        }
        let rust_type_args =
            ra_ap_syntax::ast::HasGenericArgs::generic_arg_list(&segment).map(|arguments| {
                let range = arguments.syntax().text_range();
                let arguments = &text[usize::from(range.start())..usize::from(range.end())];
                arguments.to_string()
            });
        let prefix_end = usize::from(segment.syntax().text_range().start());
        sites.push(InstantiationSite {
            start,
            end: usize::from(range.end()),
            field_values,
            field_order,
            prefix: text[start..prefix_end].to_string(),
            has_rest_pattern: false,
            rust_self: name == "Self",
            rust_type_args,
        });
    }
    (sites, blocked)
}
