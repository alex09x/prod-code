/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::parse::{parse_if_else_block, parse_rust_match, parse_switch_block};
use super::transform::{
    transform_cpp, transform_python, transform_rust, transform_swift, transform_typescript,
};
use super::types::{ReplaceConditionalResult, line_col_to_offset};

/// Orchestrator for replacing conditional with polymorphism.
#[allow(clippy::too_many_arguments)]
pub async fn replace_conditional_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    base_name: &str,
    method_name: &str,
    params: &[String],
    return_type: Option<&str>,
    target_var_opt: Option<&str>,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ReplaceConditionalResult> {
    anyhow::ensure!(
        line > 0 && col > 0,
        "replace_conditional requires one-based line and character coordinates"
    );
    let file_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;
    let language = crate::lang::language_id_for_path(file).to_string();

    let offset = line_col_to_offset(&file_text, line, col)
        .context("requested position is outside the document or splits a UTF-16 surrogate pair")?;

    // Locate the conditional block
    let block = parse_switch_block(&file_text, offset)
        .or_else(|| parse_rust_match(&file_text, offset))
        .or_else(|| parse_if_else_block(&file_text, offset))
        .with_context(|| {
            format!(
                "no switch, match, or if-else conditional block found in {} around line {line}",
                file.display()
            )
        })?;

    let target_var = target_var_opt
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .context("`target_var` is required; the discriminator is not necessarily the polymorphic receiver")?;

    let transformed_text = match language.as_str() {
        "python" => transform_python(
            &file_text,
            &block,
            base_name,
            method_name,
            params,
            target_var,
        )?,
        "typescript" | "javascript" => transform_typescript(
            &file_text,
            &block,
            base_name,
            method_name,
            params,
            return_type,
            target_var,
        )?,
        "cpp" | "c" => transform_cpp(
            &file_text,
            &block,
            base_name,
            method_name,
            params,
            return_type,
            target_var,
        )?,
        "swift" => transform_swift(
            &file_text,
            &block,
            base_name,
            method_name,
            params,
            return_type,
            target_var,
        )?,
        "rust" => transform_rust(
            &file_text,
            &block,
            base_name,
            method_name,
            params,
            return_type,
            target_var,
        )?,
        other => bail!("unsupported language for replace_conditional: {other}"),
    };

    let diff = similar::TextDiff::from_lines(&file_text, &transformed_text)
        .unified_diff()
        .context_radius(2)
        .header(&file.to_string_lossy(), &file.to_string_lossy())
        .to_string();

    let overlays = vec![(file.to_path_buf(), transformed_text.clone())];

    // Overlay validation
    let reports = crate::diagnostics::validate_texts(remote, root, &overlays, &[])
        .await
        .unwrap_or_default();
    let mut diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect();

    let mut verified = false;
    if verify == Some("compile") {
        let files_to_compile: Vec<(String, String)> = overlays
            .iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t.clone()))
            .collect();
        let check = crate::compile_check::check(remote, root, &files_to_compile).await?;
        verified = check.passed;
        if !check.passed {
            if !force {
                bail!("compiler verification failed:\n{}", check.errors.join("\n"));
            }
            diagnostics.push(format!("compiler errors: {}", check.errors.join("; ")));
        }
    }

    let has_fatal = !diagnostics.is_empty();
    if has_fatal && !force && apply {
        bail!(
            "refactoring rejected by validation:\n{}",
            diagnostics.join("\n")
        );
    }

    let applied = if apply {
        let mut file_map = BTreeMap::new();
        file_map.insert(file.to_path_buf(), transformed_text);
        let edit = crate::signature::whole_file_edit(&file_map);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        true
    } else {
        false
    };

    let variant_names = block
        .branches
        .iter()
        .map(|b| b.variant_name.clone())
        .collect();

    Ok(ReplaceConditionalResult {
        base_name: base_name.to_string(),
        method_name: method_name.to_string(),
        variants: variant_names,
        files_modified: vec![file.to_string_lossy().into_owned()],
        overlays,
        diff,
        applied,
        verified,
        diagnostics,
    })
}
