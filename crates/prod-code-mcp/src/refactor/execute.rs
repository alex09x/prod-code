/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::engine::{MultiRun, forget_synced_across_roots};
use super::history::remember_applied_multi;
use super::ops::{check_multi_ops, multi_operations};

/// Applies a `WorkspaceEdit` (`documentChanges` or `changes`) to the checkout at `root`, its
/// changes in order, as LSP says: each change names its paths as they are after the changes
/// before it. Returns the relative paths written, moved or deleted, in application order.
///
/// A multi-file refactor that stops halfway is worse than one that never started: the checkout
/// is inconsistent and nothing says which half landed. Everything is checked before the first
/// byte moves, and each step records how to take it back, renames of whole directories
/// included, so a failure puts every path and byte back as it was.
pub fn apply_workspace_edit(root: &Path, edit: &serde_json::Value) -> Result<Vec<String>> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let touched = apply_multi_repository_workspace_edit(&[&canonical_root], edit)?;
    let mut out = Vec::new();
    for p in touched {
        let rel = p
            .strip_prefix(&canonical_root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| p.to_string_lossy().replace('\\', "/"));
        out.push(rel);
    }
    Ok(out)
}

/// Applies an LSP `WorkspaceEdit` atomically across multiple repository checkouts (`roots`).
///
/// Every change in `documentChanges` or `changes` is matched to its corresponding repository root.
/// Pre-flight validation confirms file readability and deletion safety across all repositories
/// before a single byte moves. A unified transactional journal tracks all mutations across all
/// checkouts; if any change fails in any repository, every modified file, created file or directory,
/// and moved path across all repositories is rolled back to its exact original state.
///
/// On success, sync watermarks across all involved repository checkouts are invalidated so remote
/// gateways upload the updated files. Returns the absolute paths of all touched files/paths in
/// order of application.
pub fn apply_multi_repository_workspace_edit(
    roots: &[&Path],
    edit: &serde_json::Value,
) -> Result<Vec<PathBuf>> {
    anyhow::ensure!(
        !roots.is_empty(),
        "no repository roots provided for multi-repository workspace edit"
    );
    let canonical_roots: Vec<PathBuf> = roots
        .iter()
        .map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| r.to_path_buf()))
        .collect();
    for r in &canonical_roots {
        anyhow::ensure!(
            r.is_dir(),
            "repository root {} does not exist or is not a directory",
            r.display()
        );
    }
    let ops = multi_operations(&canonical_roots, edit)?;
    check_multi_ops(&ops).context("nothing was written")?;
    let mut run = MultiRun::default();
    match run.apply(&ops) {
        Ok(()) => {
            run.journal.commit();
            remember_applied_multi(&run.originals);
            forget_synced_across_roots(&canonical_roots, &run.touched, &run.also_forget);
            crate::call_tree::clear_call_hierarchy_cache();
            Ok(run.touched)
        }
        Err(err) => {
            let (restored, failed) = run.journal.roll_back();
            forget_synced_across_roots(&canonical_roots, &run.touched, &run.also_forget);
            crate::call_tree::clear_call_hierarchy_cache();
            if failed.is_empty() {
                if canonical_roots.len() == 1 {
                    Err(err.context(format!(
                        "the edit failed partway and was undone: {restored} change(s) put back as \
                         they were"
                    )))
                } else {
                    Err(err.context(format!(
                        "the edit failed partway and was undone: {restored} change(s) put back as \
                         they were across repository roots"
                    )))
                }
            } else {
                Err(err.context(format!(
                    "the edit failed partway; {restored} change(s) were put back, but these \
                     could not be:\n  {}",
                    failed.join("\n  ")
                )))
            }
        }
    }
}
