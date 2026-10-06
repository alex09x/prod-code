/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::languages::{
    extract_interface_cpp, extract_interface_go, extract_interface_python, extract_interface_swift,
    extract_interface_ts,
};
use super::types::ExtractInterfaceResult;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_arguments)]
pub async fn extract_interface_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: &str,
    interface_name: &str,
    methods: &[String],
    line: u32,
    col: u32,
    migrate_callers: bool,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ExtractInterfaceResult> {
    let language = crate::lang::language_id_for_path(file).to_string();

    // If Rust, dispatch to existing extract_trait
    if language == "rust" {
        let res = crate::extract_trait::extract_trait_ext(
            remote,
            root,
            file,
            line,
            col,
            methods,
            interface_name,
            migrate_callers,
            apply,
            force,
        )
        .await?;

        let diff = res.render();
        let files_modified: Vec<String> = res.rewritten.iter().map(|(p, _)| p.clone()).collect();
        let overlays: Vec<(PathBuf, String)> = res
            .rewritten
            .into_iter()
            .map(|(p, t)| (PathBuf::from(p), t))
            .collect();

        return Ok(ExtractInterfaceResult {
            type_name: res.type_name,
            interface_name: res.trait_name,
            methods: res.methods,
            files_modified,
            overlays,
            diff,
            applied: res.applied,
            verified: true,
            diagnostics: res.diagnostics,
        });
    }
    if verify == Some("compile") {
        anyhow::bail!(
            "verify: compile is only supported for Rust extract_interface; no files were written"
        );
    }

    let file_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;

    let (mut transformed_text, extracted_methods) = match language.as_str() {
        "typescript" => extract_interface_ts(&file_text, symbol, interface_name, methods)?,
        "javascript" => bail!(
            "extract_interface does not support JavaScript; TypeScript interface syntax cannot be emitted into JavaScript"
        ),
        "go" => extract_interface_go(&file_text, symbol, interface_name, methods)?,
        "python" => extract_interface_python(&file_text, symbol, interface_name, methods)?,
        "cpp" | "c" => extract_interface_cpp(&file_text, symbol, interface_name, methods)?,
        "swift" => extract_interface_swift(&file_text, symbol, interface_name, methods)?,
        other => bail!("unsupported language for extract_interface: {other}"),
    };

    let mut overlays: Vec<(PathBuf, String)> = Vec::new();
    let mut files_modified: Vec<String> = Vec::new();

    if migrate_callers {
        let lang = crate::parameter_object::Language::of(file)
            .unwrap_or(crate::parameter_object::Language::TypeScript);
        let modified_files = crate::caller_migration::migrate_callers_in_workspace(
            root,
            file,
            &transformed_text,
            symbol,
            interface_name,
            &extracted_methods,
            lang,
        )?;
        for (p, t) in modified_files {
            if p == *file {
                transformed_text = t.clone();
            }
            files_modified.push(p.to_string_lossy().into_owned());
            overlays.push((p, t));
        }
    }

    if overlays.is_empty() {
        overlays.push((file.to_path_buf(), transformed_text.clone()));
        files_modified.push(file.to_string_lossy().into_owned());
    }

    let mut diff = String::new();
    for (p, new_t) in &overlays {
        let orig = if *p == *file {
            file_text.clone()
        } else {
            std::fs::read_to_string(p).unwrap_or_default()
        };
        let d = similar::TextDiff::from_lines(&orig, new_t)
            .unified_diff()
            .context_radius(2)
            .header(&p.to_string_lossy(), &p.to_string_lossy())
            .to_string();
        if !d.is_empty() {
            diff.push_str(&d);
        }
    }

    // Overlay validation
    let reports = crate::diagnostics::validate_texts(remote, root, &overlays, &[]).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.source
                    .as_deref()
                    .map(|s| format!("[{s}] "))
                    .unwrap_or_default(),
                d.message,
                d.line,
                d.col
            )
        })
        .collect();

    let verified = false;

    let has_fatal = !diagnostics.is_empty();
    if has_fatal && !force && apply {
        bail!(
            "refactoring rejected by validation:\n{}",
            diagnostics.join("\n")
        );
    }

    let applied = if apply {
        let mut file_map = BTreeMap::new();
        for (p, t) in &overlays {
            file_map.insert(p.clone(), t.clone());
        }
        let edit = crate::signature::whole_file_edit(&file_map);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        true
    } else {
        false
    };

    Ok(ExtractInterfaceResult {
        type_name: symbol.to_string(),
        interface_name: interface_name.to_string(),
        methods: extracted_methods,
        files_modified,
        overlays,
        diff,
        applied,
        verified,
        diagnostics,
    })
}
