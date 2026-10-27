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
    extract_interface_cpp, extract_interface_go, extract_interface_js, extract_interface_python,
    extract_interface_swift, extract_interface_ts,
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
    let compile = verify == Some("compile");

    // If Rust, dispatch to existing extract_trait
    if language == "rust" {
        // With `verify: compile`, extract_trait only plans; the compiler sees the result before
        // anything is written (#1462).
        let res = crate::extract_trait::extract_trait_ext(
            remote,
            root,
            file,
            line,
            col,
            methods,
            interface_name,
            migrate_callers,
            apply && !compile,
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
        let mut diagnostics = res.diagnostics;
        let mut applied = res.applied;
        let mut verified = false;
        if compile {
            verified = compiler_accepts(remote, root, &overlays, force, &mut diagnostics).await?;
            if apply {
                anyhow::ensure!(
                    diagnostics.is_empty() || force,
                    "the change does not compile ({} error(s)); nothing was written:\n  {}",
                    diagnostics.len(),
                    diagnostics.join("\n  ")
                );
                write_proposal(root, &overlays)?;
                applied = true;
            }
        }

        return Ok(ExtractInterfaceResult {
            type_name: res.type_name,
            interface_name: res.trait_name,
            methods: res.methods,
            files_modified,
            overlays,
            diff,
            applied,
            verified,
            diagnostics,
        });
    }

    let file_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;

    let (mut transformed_text, extracted_methods) = match language.as_str() {
        "typescript" => extract_interface_ts(&file_text, symbol, interface_name, methods)?,
        "javascript" => extract_interface_js(&file_text, symbol, interface_name, methods)?,
        "go" => extract_interface_go(&file_text, symbol, interface_name, methods)?,
        "python" => extract_interface_python(&file_text, symbol, interface_name, methods)?,
        "cpp" | "c" => extract_interface_cpp(&file_text, symbol, interface_name, methods)?,
        "swift" => extract_interface_swift(&file_text, symbol, interface_name, methods)?,
        other => bail!("unsupported language for extract_interface: {other}"),
    };

    let mut overlays: Vec<(PathBuf, String)> = Vec::new();
    let mut files_modified: Vec<String> = Vec::new();

    // A JavaScript caller declares no parameter types to migrate.
    if migrate_callers && language != "javascript" {
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
    let mut diagnostics = diagnostics;

    // The project's own compiler, chosen by language and project (#1462).
    let verified = if compile {
        compiler_accepts(remote, root, &overlays, force, &mut diagnostics).await?
    } else {
        false
    };

    let has_fatal = !diagnostics.is_empty();
    if has_fatal && !force && apply {
        bail!(
            "refactoring rejected by validation:\n{}",
            diagnostics.join("\n")
        );
    }

    let applied = if apply {
        write_proposal(root, &overlays)?;
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

/// Runs the project's compiler on `overlays` in a shadow of the workspace: `cargo check`, `go
/// build`, `tsc` and the others, chosen by the files' language and project. Whether it accepts
/// them; its errors are added to `diagnostics`, and refuse the change unless `force` (#1462).
async fn compiler_accepts(
    remote: SocketAddr,
    root: &Path,
    overlays: &[(PathBuf, String)],
    force: bool,
    diagnostics: &mut Vec<String>,
) -> Result<bool> {
    let (errors, report) = crate::tools::compile_check(remote, root, overlays).await?;
    if errors == 0 {
        return Ok(true);
    }
    if !force {
        bail!("compiler verification failed ({errors} error(s)); no files were written:\n{report}");
    }
    diagnostics.push(format!("compiler: {errors} error(s): {report}"));
    Ok(false)
}

/// Writes the proposed texts.
fn write_proposal(root: &Path, overlays: &[(PathBuf, String)]) -> Result<()> {
    let files: BTreeMap<PathBuf, String> = overlays.iter().cloned().collect();
    crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
    Ok(())
}
