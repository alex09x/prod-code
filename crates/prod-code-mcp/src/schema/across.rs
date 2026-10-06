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

use anyhow::Result;

use super::edits::{NOT_FOUND, make_workspace_edit_with_ends};
use super::execute::rename;
use super::types::AcrossRepos;

/// Renames a schema field in several repositories as one change.
///
/// Each repository is planned and checked by its own analyzers, exactly as [`rename`] does for
/// one. Nothing is written until every plan is ready, and with `apply` nothing is written unless
/// every repository accepts its result (or `force`). The repositories are then written one
/// after another; when one of them cannot be written, the ones already written are put back
/// as they were, so the change lands in all of them or in none.
pub async fn rename_across(
    remote: SocketAddr,
    roots: &[PathBuf],
    field: &str,
    to: &str,
    apply: bool,
    force: bool,
) -> Result<AcrossRepos> {
    let mut repos = Vec::new();
    let mut missing = Vec::new();
    for root in roots {
        match rename(remote, root, field, to, false, force, None).await {
            Ok(plan) => repos.push(plan),
            Err(err) if format!("{err}").contains(NOT_FOUND) => missing.push(root.clone()),
            Err(err) => return Err(err.context(format!("in {}", root.display()))),
        }
    }
    anyhow::ensure!(
        !repos.is_empty(),
        "`{field}` does not appear in any of the {} repositories",
        roots.len()
    );
    let mut applied = false;
    if apply {
        let errors: Vec<String> = repos
            .iter()
            .flat_map(|r| {
                r.diagnostics
                    .iter()
                    .map(move |d| format!("{}: {d}", r.root.display()))
            })
            .collect();
        anyhow::ensure!(
            errors.is_empty() || force,
            "the rename does not compile ({} error(s)); nothing was written in any repository. \
             Pass `force: true` to write it anyway:\n  {}",
            errors.len(),
            errors.join("\n  ")
        );
        let repo_roots: Vec<&Path> = repos.iter().map(|r| r.root.as_path()).collect();
        let mut all_ends = BTreeMap::new();
        let mut all_rewritten = Vec::new();
        for r in &repos {
            all_ends.extend(r.original_ends.clone());
            all_rewritten.extend(r.rewritten.clone());
        }
        let multi_edit = make_workspace_edit_with_ends(&all_rewritten, &all_ends);
        crate::refactor::apply_multi_repository_workspace_edit(&repo_roots, &multi_edit)?;
        for repo in &mut repos {
            repo.applied = true;
        }
        applied = true;
    }
    Ok(AcrossRepos {
        repos,
        missing,
        applied,
    })
}
