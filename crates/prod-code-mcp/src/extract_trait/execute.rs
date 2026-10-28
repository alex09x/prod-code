/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::move_item::{CargoTargetModule, cargo_target_module};
use anyhow::{Context, Result};

use super::items::impl_block;
use super::rewrite::{
    add_import_at_inline_scope, inline_scope_at, inline_scope_id_at, inline_scope_open_by_id,
    relative_import_path, rewrite,
};
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
    let (trait_inline_modules, _) = inline_scope_at(&text, imp.start);

    // Every reference to a moved method needs the trait in its caller's actual module scope.
    let source_target = crate::move_item::cargo_target_module(file)?;
    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    files.insert(file.to_path_buf(), new_text.clone());
    let mut imports = Vec::new();
    let mut caller_targets = BTreeMap::new();
    let mut pending_imports: BTreeMap<PathBuf, BTreeSet<(usize, String)>> = BTreeMap::new();
    let rewritten_delta = new_text.len() as isize - text.len() as isize;
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
        for (path, ref_line, ref_col) in
            crate::signature::references(remote, root, file, l, c).await?
        {
            if path == file {
                let caller_at = crate::signature::offset_of(&text, ref_line, ref_col)
                    .context("a same-file reference is outside its source text")?;
                if (imp.start..imp.close).contains(&caller_at) {
                    continue;
                }
                let (caller_modules, _) = inline_scope_at(&text, caller_at);
                let Some(relative) =
                    relative_import_path(&caller_modules, &trait_inline_modules, name)
                else {
                    continue;
                };
                let rewritten_at = if caller_at >= imp.close {
                    usize::try_from(caller_at as isize + rewritten_delta)
                        .context("rewritten same-file reference position is invalid")?
                } else {
                    caller_at
                };
                let scope_id = inline_scope_id_at(&new_text, rewritten_at);
                pending_imports
                    .entry(file.to_path_buf())
                    .or_default()
                    .insert((scope_id, format!("use {relative};")));
                continue;
            }
            let caller_target =
                cached_cargo_target(&path, &mut caller_targets).with_context(|| {
                    format!(
                        "cannot identify Cargo target for caller {}; no trait import was planned",
                        path.display()
                    )
                })?;
            let mut target_module = if caller_target.identity == source_target.identity {
                source_target
                    .module
                    .spelled_from(&source_target.module.krate)
            } else {
                source_target.module.absolute()
            };
            if !trait_inline_modules.is_empty() {
                target_module.push_str("::");
                target_module.push_str(&trait_inline_modules.join("::"));
            }
            let use_line = format!("use {target_module}::{name};");
            let current = match files.get(&path) {
                Some(t) => t.clone(),
                None => std::fs::read_to_string(&path)
                    .with_context(|| format!("cannot read {}", path.display()))?,
            };
            let caller_at = crate::signature::offset_of(&current, ref_line, ref_col)
                .context("a caller reference is outside its source text")?;
            let scope_id = inline_scope_id_at(&current, caller_at);
            pending_imports
                .entry(path.clone())
                .or_default()
                .insert((scope_id, use_line));
            files.insert(path, current);
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
                let caller_target = cached_cargo_target(&p, &mut caller_targets).with_context(|| {
                    format!(
                        "cannot identify Cargo target for migrated caller {}; no trait import was planned",
                        p.display()
                    )
                })?;
                let mut target_module = if caller_target.identity == source_target.identity {
                    source_target
                        .module
                        .spelled_from(&source_target.module.krate)
                } else {
                    source_target.module.absolute()
                };
                if !trait_inline_modules.is_empty() {
                    target_module.push_str("::");
                    target_module.push_str(&trait_inline_modules.join("::"));
                }
                let use_line = format!("use {target_module}::{name};");
                let has_root_import = pending_imports.get(&p).is_some_and(|planned| {
                    planned
                        .iter()
                        .any(|(scope_id, line)| *scope_id == 0 && line == &use_line)
                });
                if !has_root_import {
                    pending_imports
                        .entry(p.clone())
                        .or_default()
                        .insert((0, use_line));
                }
                files.insert(p, t);
            } else {
                files.insert(p, t);
            }
        }
    }

    for (path, caller_imports) in pending_imports {
        let Some(current) = files.get(&path).cloned() else {
            continue;
        };
        let mut updated = current;
        for (scope_id, use_line) in caller_imports.into_iter().rev() {
            let scope_open = inline_scope_open_by_id(&updated, scope_id)
                .context("cannot relocate a planned inline-module import after caller edits")?;
            let with_import = add_import_at_inline_scope(&updated, scope_open, &use_line);
            if with_import != updated {
                imports.push((path.clone(), use_line));
                updated = with_import;
            }
        }
        files.insert(path, updated);
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

fn cached_cargo_target(
    path: &Path,
    cache: &mut BTreeMap<PathBuf, CargoTargetModule>,
) -> Result<CargoTargetModule> {
    if let Some(target) = cache.get(path) {
        return Ok(target.clone());
    }
    let target = cargo_target_module(path)?;
    cache.insert(path.to_path_buf(), target.clone());
    Ok(target)
}
