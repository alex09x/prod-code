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
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::codegen::{generate_builder_code, generate_factory_code};
use super::parse::parse_struct_declaration;
use super::rewrite::rewrite_instantiation;
use super::sites::{
    find_cpp_instantiations, find_go_instantiations, find_python_instantiations,
    find_rust_instantiations, find_swift_instantiations, find_ts_instantiations,
};
use super::types::{ReplaceConstructorResult, ReplaceMode, display};

/// Core implementation for replacing constructor with factory or builder.
#[allow(clippy::too_many_arguments)]
pub async fn replace_constructor_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    type_name: &str,
    mode: ReplaceMode,
    target_name_opt: Option<&str>,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ReplaceConstructorResult> {
    let decl_text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let language = crate::lang::language_id_for_path(file).to_string();

    let decl = parse_struct_declaration(&decl_text, type_name, &language)?;
    let target_name = match target_name_opt {
        Some(n) => n.to_string(),
        None => match mode {
            ReplaceMode::Factory => match language.as_str() {
                "rust" => "new".to_string(),
                "go" => format!("New{type_name}"),
                _ => "create".to_string(),
            },
            ReplaceMode::Builder => format!("{type_name}Builder"),
        },
    };

    let generated_code = match mode {
        ReplaceMode::Factory => generate_factory_code(&decl, &target_name),
        ReplaceMode::Builder => generate_builder_code(&decl, &target_name),
    };

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), decl_text.clone());

    // Edits per file: (byte_offset, replace_len, replacement)
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();

    // 1. Add generated factory / builder declaration to the defining file
    let insert_at = if mode == ReplaceMode::Factory
        && matches!(
            language.as_str(),
            "typescript"
                | "typescriptreact"
                | "javascript"
                | "javascriptreact"
                | "cpp"
                | "c"
                | "swift"
        ) {
        decl.decl_end.saturating_sub(1)
    } else if mode == ReplaceMode::Builder
        && matches!(language.as_str(), "cpp" | "c")
        && decl_text.as_bytes().get(decl.decl_end) == Some(&b';')
    {
        decl.decl_end + 1
    } else {
        decl.decl_end
    };
    edits
        .entry(file.to_path_buf())
        .or_default()
        .push((insert_at, 0, generated_code));

    let mut all_blocked = Vec::new();
    let mut instantiations_rewritten = 0usize;

    // 2. Discover references to `type_name` across workspace
    let lsp_refs = crate::signature::references(remote, root, file, decl.line, decl.col)
        .await
        .unwrap_or_default();

    let mut target_files = BTreeMap::new();
    target_files.insert(file.to_path_buf(), ());
    for (ref_path, _, _) in lsp_refs {
        target_files.insert(ref_path, ());
    }

    // 3. Find and rewrite instantiations in all candidate files
    for (path, _) in target_files {
        let body = crate::refactor::referenced_text(&mut texts, &path)?.clone();
        let (sites, blocked) = match language.as_str() {
            "rust" => {
                let (s, b) = find_rust_instantiations(
                    &body,
                    type_name,
                    if path == file { decl.decl_start } else { 0 },
                    if path == file { decl.decl_end } else { 0 },
                );
                (s, b)
            }
            "go" => (find_go_instantiations(&body, type_name, 0, 0), Vec::new()),
            "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => {
                (find_ts_instantiations(&body, type_name, 0, 0), Vec::new())
            }
            "python" => (
                find_python_instantiations(&body, type_name, 0, 0),
                Vec::new(),
            ),
            "cpp" | "c" => (
                find_cpp_instantiations(
                    &body,
                    type_name,
                    if path == file { decl.decl_start } else { 0 },
                    if path == file { decl.decl_end } else { 0 },
                ),
                Vec::new(),
            ),
            "swift" => (
                find_swift_instantiations(
                    &body,
                    type_name,
                    if path == file { decl.decl_start } else { 0 },
                    if path == file { decl.decl_end } else { 0 },
                ),
                Vec::new(),
            ),
            _ => (Vec::new(), Vec::new()),
        };

        all_blocked.extend(
            blocked
                .into_iter()
                .map(|b| format!("{}: {b}", display(root, &path))),
        );

        for site in sites {
            match rewrite_instantiation(&site, &decl, mode, &target_name) {
                Ok(replacement) => {
                    edits.entry(path.clone()).or_default().push((
                        site.start,
                        site.end - site.start,
                        replacement,
                    ));
                    instantiations_rewritten += 1;
                }
                Err(err) => {
                    all_blocked.push(format!("{} at {}: {err}", display(root, &path), site.start));
                }
            }
        }
    }

    // 4. Materialize rewritten texts in memory
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, _, _)| *at);
        for (at, len, replacement) in file_edits.into_iter().rev() {
            if at + len <= body.len() {
                body.replace_range(at..at + len, &replacement);
            }
        }
        rewritten.insert(path, body);
    }

    // 5. Validate proposed edits against analyzer
    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, &[]).await?;
    let diagnostics: Vec<String> = reports
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

    // 6. Optional compiler check for Rust
    if verify == Some("compile") && language == "rust" {
        let files: Vec<(String, String)> = rewritten
            .iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t.clone()))
            .collect();
        let check = crate::compile_check::check(remote, root, &files).await?;
        if !check.passed {
            anyhow::bail!(
                "cargo check failed on the workspace: {} error(s):\n  {}",
                check.errors.len(),
                check.errors.join("\n  ")
            );
        }
    }

    // 7. Apply if requested and safe
    let mut applied = false;
    if apply {
        anyhow::ensure!(
            all_blocked.is_empty(),
            "{} instantiation(s) could not be safely rewritten; nothing was written:\n  {}",
            all_blocked.len(),
            all_blocked.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the refactoring produces {} diagnostic error(s); nothing was written. Pass `force: true` to bypass:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(ReplaceConstructorResult {
        type_name: type_name.to_string(),
        root: root.to_path_buf(),
        file: display(root, file),
        mode,
        target_name,
        declared_fields: decl
            .fields
            .iter()
            .map(|f| format!("{}: {}", f.name, f.ty))
            .collect(),
        instantiations_rewritten,
        blocked: all_blocked,
        unmatched: Vec::new(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
        language,
    })
}

/// Replace constructor/instantiations with a static factory method.
#[allow(clippy::too_many_arguments)]
pub async fn replace_constructor_with_factory(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    type_name: &str,
    factory_name: Option<&str>,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ReplaceConstructorResult> {
    replace_constructor_impl(
        remote,
        root,
        file,
        type_name,
        ReplaceMode::Factory,
        factory_name,
        apply,
        force,
        verify,
    )
    .await
}

/// Replace constructor/instantiations with a fluent builder pattern.
#[allow(clippy::too_many_arguments)]
pub async fn replace_constructor_with_builder(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    type_name: &str,
    builder_name: Option<&str>,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ReplaceConstructorResult> {
    replace_constructor_impl(
        remote,
        root,
        file,
        type_name,
        ReplaceMode::Builder,
        builder_name,
        apply,
        force,
        verify,
    )
    .await
}
