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

use super::items::impl_block;
use super::rewrite::rewrite;
use super::types::{Extracted, valid_ident};

/// Extracts `trait {name}` from the methods `methods` of the inherent `impl` block at
/// `line`:`col` of `file`.
#[allow(clippy::too_many_arguments)]
pub async fn extract_trait(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    methods: &[String],
    name: &str,
    apply: bool,
    force: bool,
) -> Result<Extracted> {
    extract_trait_ext(
        remote, root, file, line, col, methods, name, false, apply, force,
    )
    .await
}

/// Extracts `trait {name}` with optional caller type annotation migration.
#[allow(clippy::too_many_arguments)]
pub async fn extract_trait_ext(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    methods: &[String],
    name: &str,
    migrate_callers: bool,
    apply: bool,
    force: bool,
) -> Result<Extracted> {
    anyhow::ensure!(valid_ident(name), "`{name}` is not a valid Rust identifier");
    anyhow::ensure!(!methods.is_empty(), "name at least one method");
    for method in methods {
        anyhow::ensure!(
            valid_ident(method),
            "`{method}` is not a valid Rust method identifier"
        );
    }
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at =
        crate::signature::offset_of(&text, line, col).context("the position is not in the file")?;
    let imp = impl_block(&text, at)?;
    let (new_text, kept) = rewrite(&text, &imp, methods, name)?;

    // Every file outside this one that calls a moved method needs the trait in scope.
    let (_, module) = crate::move_item::module_of(file)?;
    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    files.insert(file.to_path_buf(), new_text.clone());
    let mut imports = Vec::new();
    for item in imp
        .items
        .iter()
        .filter(|i| i.name.as_ref().is_some_and(|n| methods.contains(n)))
    {
        let fn_name = item.name.as_deref().unwrap_or_default();
        let name_at = item.start
            + text[item.start..item.end]
                .find(&format!("fn {fn_name}"))
                .map_or(0, |i| i + 3);
        let (l, c) = crate::signature::position_at(&text, name_at)?;
        for (path, _, _) in crate::signature::references(remote, root, file, l, c).await? {
            if path == file {
                continue;
            }
            let caller_crate = crate::move_item::module_of(&path)
                .map(|(_, m)| m.krate)
                .unwrap_or_else(|_| module.krate.clone());
            let use_line = format!("use {}::{name};", module.spelled_from(&caller_crate));
            let current = match files.get(&path) {
                Some(t) => t.clone(),
                None => std::fs::read_to_string(&path)
                    .with_context(|| format!("cannot read {}", path.display()))?,
            };
            let updated = crate::move_item::add_import(&current, &use_line);
            if updated != current {
                imports.push((path.clone(), use_line));
                files.insert(path, updated);
            }
        }
    }

    if migrate_callers && imp.generics.is_empty() && !imp.self_ty.contains('<') {
        let current_text = files.get(file).cloned().unwrap_or(new_text);
        let modified_files = crate::caller_migration::migrate_callers_in_workspace(
            root,
            file,
            &current_text,
            &imp.self_ty,
            name,
            methods,
            crate::parameter_object::Language::Rust,
        )?;
        for (p, t) in modified_files {
            if p != file {
                let caller_crate = crate::move_item::module_of(&p)
                    .map(|(_, m)| m.krate)
                    .unwrap_or_else(|_| module.krate.clone());
                let use_line = format!("use {}::{name};", module.spelled_from(&caller_crate));
                let updated = crate::move_item::add_import(&t, &use_line);
                if updated != t && !imports.iter().any(|(imp_p, _)| imp_p == &p) {
                    imports.push((p.clone(), use_line));
                }
                files.insert(p, updated);
            } else {
                files.insert(p, t);
            }
        }
    }

    let edits: Vec<(PathBuf, String)> = files.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &[]).await?;
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
    let mut applied = false;
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }
    let rel = |p: &Path| p.strip_prefix(root).unwrap_or(p).display().to_string();
    Ok(Extracted {
        trait_name: name.to_string(),
        type_name: imp.self_ty.clone(),
        root: root.to_path_buf(),
        file: rel(file),
        methods: methods.to_vec(),
        kept,
        imports: imports.iter().map(|(p, l)| (rel(p), l.clone())).collect(),
        rewritten: files
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
