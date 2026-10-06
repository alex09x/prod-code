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

use super::edits::{merge, minimal_edits, text_edits};
use super::git::create_prune_commit;
use super::types::Pruned;

/// Removes every orphan the dead-code scan finds in the checkout at `root`.
pub async fn prune_orphans(
    remote: SocketAddr,
    root: &Path,
    max_files: usize,
    apply: bool,
    force: bool,
) -> Result<Pruned> {
    prune_orphans_opts(
        remote,
        root,
        crate::dead_code::DeadCodeOptions {
            include_exported: false,
            max_files,
            reachability: false,
        },
        apply,
        force,
        false,
        false,
    )
    .await
}

/// Removes every orphan found in the checkout at `root` using the specified options
/// (including whole-program reachability analysis, Git commit patch generation, and optional Git commit creation).
pub async fn prune_orphans_opts(
    remote: SocketAddr,
    root: &Path,
    options: crate::dead_code::DeadCodeOptions,
    mut apply: bool,
    force: bool,
    git_patch: bool,
    commit: bool,
) -> Result<Pruned> {
    if commit {
        apply = true;
    }
    let report = crate::dead_code::find_dead_code_opts(remote, root, options).await?;
    let mut merged: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    let mut removed = Vec::new();
    let mut skipped = Vec::new();
    for item in report.dead {
        let file = root.join(&item.file);
        let uri = url::Url::from_file_path(&file)
            .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
            .to_string();
        let answer = crate::tools::execute_lsp_query(
            remote,
            root,
            &file,
            "prodCode/safeDelete",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": item.line.saturating_sub(1), "character": item.col.saturating_sub(1) }
            }),
        )
        .await;
        let deletion = match answer.as_ref().ok().and_then(text_edits) {
            Some(d) if !d.is_empty() => d,
            Some(_) => {
                skipped.push((item, "safe delete produced no edit".to_string()));
                continue;
            }
            None => {
                let why = match answer {
                    Err(e) => format!("safe delete refused: {e:#}"),
                    Ok(_) => "safe delete would move or create files".to_string(),
                };
                skipped.push((item, why));
                continue;
            }
        };
        // Each answer, reduced to the lines it changes in the file as it is.
        let mut reduced = Vec::with_capacity(deletion.len());
        for (uri, edits) in deletion {
            let path = crate::remote_fs::uri_to_path(&uri);
            let old = std::fs::read_to_string(&path).unwrap_or_default();
            let new = crate::refactor::apply_text_edits(&old, &edits)?;
            reduced.push((uri, minimal_edits(&old, &new)));
        }
        if merge(&mut merged, reduced) {
            removed.push(item);
        } else {
            skipped.push((item, "it overlaps another removal; run again".to_string()));
        }
    }
    let edit = serde_json::json!({ "changes": merged });
    let (texts, _) = crate::refactor::planned_texts(root, &edit)?;
    let mut diagnostics = Vec::new();
    if !texts.is_empty() {
        let reports = crate::diagnostics::validate_texts(remote, root, &texts, &[]).await?;
        diagnostics = reports
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
    }
    let files: BTreeMap<PathBuf, String> = texts.into_iter().collect();
    let mut git_dir = None;
    if commit && !files.is_empty() {
        let git_dir_out = std::process::Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "--absolute-git-dir"])
            .output()
            .context("failed to execute git rev-parse --absolute-git-dir")?;
        anyhow::ensure!(
            git_dir_out.status.success(),
            "cannot create git commit: {} is not inside a git repository",
            root.display()
        );
        let resolved_git_dir = PathBuf::from(String::from_utf8_lossy(&git_dir_out.stdout).trim());
        anyhow::ensure!(
            resolved_git_dir.is_dir(),
            "cannot create git commit: resolved git directory {} is not a directory",
            resolved_git_dir.display()
        );
        git_dir = Some(resolved_git_dir);

        let head_out = std::process::Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "--verify", "HEAD"])
            .output()
            .context("failed to execute git rev-parse HEAD")?;
        anyhow::ensure!(
            head_out.status.success(),
            "cannot create git commit: repository has no commits on HEAD"
        );
        let mut diff_cmd = std::process::Command::new("git");
        diff_cmd
            .current_dir(root)
            .args(["diff-index", "--name-only", "HEAD", "--"]);
        for path in files.keys() {
            diff_cmd.arg(path);
        }
        let diff_out = diff_cmd
            .output()
            .context("failed to execute git diff-index")?;
        anyhow::ensure!(
            diff_out.status.success(),
            "failed to check git status on HEAD"
        );
        let dirty = String::from_utf8_lossy(&diff_out.stdout).trim().to_string();
        anyhow::ensure!(
            dirty.is_empty(),
            "cannot create git commit: touched file(s) have uncommitted changes relative to HEAD:\n  {}",
            dirty.lines().collect::<Vec<_>>().join("\n  ")
        );
    }

    let mut applied = false;
    if apply && !files.is_empty() {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the pruned checkout does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }

    let mut pruned = Pruned {
        root: root.to_path_buf(),
        removed,
        skipped,
        rewritten: files
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
        symbols_checked: report.symbols_checked,
        unverified: report.unverified,
        git_patch: None,
        git_commit: None,
    };

    let patch_text = pruned.generate_git_patch();
    if git_patch {
        pruned.git_patch = patch_text.clone();
    }

    if commit && applied && !pruned.rewritten.is_empty() {
        if let (Some(patch), Some(git_dir)) = (patch_text, git_dir) {
            create_prune_commit(root, &git_dir, &mut pruned, &patch)?;
        }
    }

    Ok(pruned)
}
