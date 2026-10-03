//! Pruning orphans (roadmap 8.6): every function and type the dead-code scan finds unreferenced
//! is removed with the analyzer's safe delete, all in one edit that is type-checked before
//! anything is written.
//!
//! Only what the scan calls dead is taken: exported items (something outside the checkout may
//! use them) and methods that may be reached through a trait are left alone. Each deletion is
//! computed against the checkout as it is; one that overlaps another is left for the next run.
//! Removing a function can orphan the functions only it called, so a second run may find more.
//! A symbol whose references the analyzer did not establish is never on that list (#435): it is
//! reported as unverified and kept.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::dead_code::DeadItem;

/// What the pruning did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Pruned {
    #[serde(skip)]
    pub root: PathBuf,
    pub removed: Vec<DeadItem>,
    /// Items the scan listed that were not removed, and why.
    pub skipped: Vec<(DeadItem, String)>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
    pub symbols_checked: usize,
    /// What the scan could not judge, kept whatever it is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unverified: Vec<crate::dead_code::Unverified>,
    /// Formatted Git commit patch (git apply / git am compatible)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_patch: Option<String>,
    /// Created Git commit SHA, if commit was requested
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
}

impl Pruned {
    pub fn render(&self) -> String {
        let mut out = format!(
            "{} orphan(s) of {} symbol(s) checked\n",
            self.removed.len(),
            self.symbols_checked
        );
        for d in &self.removed {
            out.push_str(&format!(
                "  - {} {} ({}:{})\n",
                d.kind, d.name, d.file, d.line
            ));
        }
        for (d, why) in &self.skipped {
            out.push_str(&format!(
                "  kept {} {} ({}:{}): {why}\n",
                d.kind, d.name, d.file, d.line
            ));
        }
        for u in &self.unverified {
            match &u.name {
                Some(name) => out.push_str(&format!(
                    "  kept {name} ({}:{}): its references are unknown: {}\n",
                    u.file, u.line, u.reason
                )),
                None => out.push_str(&format!(
                    "  kept everything in {}: its symbols are unknown: {}\n",
                    u.file, u.reason
                )),
            }
        }
        for (path, new_text) in &self.rewritten {
            let full = Path::new(path);
            let old_text = if self.applied {
                crate::refactor::text_before_apply(full)
            } else {
                std::fs::read_to_string(full).unwrap_or_default()
            };
            let rel = full
                .strip_prefix(&self.root)
                .unwrap_or(full)
                .to_string_lossy()
                .replace('\\', "/");
            out.push('\n');
            out.push_str(
                &similar::TextDiff::from_lines(old_text.as_str(), new_text.as_str())
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        if self.removed.is_empty() {
            out.push_str(if self.unverified.is_empty() {
                "\nnothing to prune\n"
            } else {
                "\nnothing proven orphaned; what could not be checked was kept\n"
            });
            return out;
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        if let Some(commit) = &self.git_commit {
            out.push_str(&format!(
                "\n[committed: {commit}] created Git commit with author Alexander Panasenko <alex@prod.codes>\n"
            ));
        } else if self.applied {
            out.push_str("\n[applied] a second run may find what these removals orphaned\n");
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make this edit\n");
        }
        if let Some(patch) = &self.git_patch {
            out.push_str("\n--- Git Commit Patch ---\n");
            out.push_str(patch);
        }
        out
    }

    /// Generates a standard Git commit patch (compatible with `git apply` and `git am`).
    pub fn generate_git_patch(&self) -> Option<String> {
        if self.rewritten.is_empty() || self.removed.is_empty() {
            return None;
        }
        let mut patch = String::new();
        let commit_subject = format!(
            "refactor(prune): remove {} unreferenced orphan(s)",
            self.removed.len()
        );
        patch.push_str("From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001\n");
        patch.push_str("From: Alexander Panasenko <alex@prod.codes>\n");
        patch.push_str("Date: Fri, 2 Oct 2026 20:00:00 +0000\n");
        patch.push_str(&format!("Subject: [PATCH] {commit_subject}\n\n"));
        patch.push_str(&format!(
            "Pruned {} orphan(s) of {} symbol(s) checked:\n",
            self.removed.len(),
            self.symbols_checked
        ));
        for d in &self.removed {
            patch.push_str(&format!("  - {} {} ({}:{})\n", d.kind, d.name, d.file, d.line));
        }
        patch.push_str("\n---\n");

        let mut total_added = 0usize;
        let mut total_deleted = 0usize;
        let mut file_diffs = Vec::new();

        for (path, new_text) in &self.rewritten {
            let full = Path::new(path);
            let old_text = if self.applied {
                crate::refactor::text_before_apply(full)
            } else {
                std::fs::read_to_string(full).unwrap_or_default()
            };
            let rel = full
                .strip_prefix(&self.root)
                .unwrap_or(full)
                .to_string_lossy()
                .replace('\\', "/");

            let diff = similar::TextDiff::from_lines(old_text.as_str(), new_text.as_str());
            let mut added = 0usize;
            let mut deleted = 0usize;
            for change in diff.iter_all_changes() {
                match change.tag() {
                    similar::ChangeTag::Insert => added += 1,
                    similar::ChangeTag::Delete => deleted += 1,
                    similar::ChangeTag::Equal => {}
                }
            }
            total_added += added;
            total_deleted += deleted;

            let unified = diff
                .unified_diff()
                .context_radius(3)
                .header(&format!("a/{rel}"), &format!("b/{rel}"))
                .to_string();

            file_diffs.push((rel, added, deleted, unified));
        }

        for (rel, added, deleted, _) in &file_diffs {
            let count = added + deleted;
            let plus_bar = "+".repeat((*added).min(20));
            let minus_bar = "-".repeat((*deleted).min(20));
            patch.push_str(&format!(" {:<35} | {:>4} {plus_bar}{minus_bar}\n", rel, count));
        }
        let file_s = if file_diffs.len() == 1 { "file" } else { "files" };
        let ins_s = if total_added == 1 { "insertion" } else { "insertions" };
        let del_s = if total_deleted == 1 { "deletion" } else { "deletions" };
        patch.push_str(&format!(
            " {} {} changed, {} {}(+), {} {}(-)\n\n",
            file_diffs.len(),
            file_s,
            total_added,
            ins_s,
            total_deleted,
            del_s
        ));

        for (rel, _, _, unified) in file_diffs {
            patch.push_str(&format!("diff --git a/{rel} b/{rel}\n"));
            patch.push_str(&unified);
        }
        patch.push_str("-- \nprod-code\n");

        Some(patch)
    }
}

/// The text edits of a `WorkspaceEdit`, per document URI; None when it also creates, renames or
/// deletes files, which a deletion of an item should never do.
pub fn text_edits(edit: &serde_json::Value) -> Option<Vec<(String, Vec<serde_json::Value>)>> {
    let mut out = Vec::new();
    if let Some(changes) = edit.get("documentChanges").and_then(|c| c.as_array()) {
        for change in changes {
            if change.get("kind").is_some() {
                return None;
            }
            let uri = change.pointer("/textDocument/uri")?.as_str()?.to_string();
            out.push((uri, change.get("edits")?.as_array()?.clone()));
        }
    } else if let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) {
        for (uri, edits) in changes {
            out.push((uri.clone(), edits.as_array()?.clone()));
        }
    }
    Some(out)
}

/// The edits that turn `old` into `new`, one per changed run of lines. The analyzer may answer
/// a deletion with the whole file replaced; two such answers always overlap, while the lines
/// each one really changes usually do not.
pub fn minimal_edits(old: &str, new: &str) -> Vec<serde_json::Value> {
    let diff = similar::TextDiff::from_lines(old, new);
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    diff.ops()
        .iter()
        .filter(|op| op.tag() != similar::DiffTag::Equal)
        .map(|op| {
            let (o, n) = (op.old_range(), op.new_range());
            serde_json::json!({
                "range": {
                    "start": { "line": o.start, "character": 0 },
                    "end": { "line": o.end, "character": 0 }
                },
                "newText": new_lines[n.start..n.end].concat()
            })
        })
        .collect()
}

/// The (start, end) of an LSP range as (line, character) pairs.
fn span(edit: &serde_json::Value) -> Option<((u64, u64), (u64, u64))> {
    let at = |p: &str| {
        Some((
            edit.pointer(&format!("/range/{p}/line"))?.as_u64()?,
            edit.pointer(&format!("/range/{p}/character"))?.as_u64()?,
        ))
    };
    Some((at("start")?, at("end")?))
}

/// Adds the edits of one deletion to `merged` unless one of them overlaps an edit already there.
pub fn merge(
    merged: &mut BTreeMap<String, Vec<serde_json::Value>>,
    deletion: Vec<(String, Vec<serde_json::Value>)>,
) -> bool {
    for (uri, edits) in &deletion {
        let taken = merged.get(uri).map(|v| v.as_slice()).unwrap_or(&[]);
        for e in edits {
            let Some((s, en)) = span(e) else {
                return false;
            };
            if taken
                .iter()
                .filter_map(span)
                .any(|(ts, ten)| s < ten && ts < en.max(s))
            {
                return false;
            }
        }
    }
    for (uri, edits) in deletion {
        merged.entry(uri).or_default().extend(edits);
    }
    true
}

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
        diff_cmd.current_dir(root).args(["diff-index", "--name-only", "HEAD", "--"]);
        for path in files.keys() {
            diff_cmd.arg(path);
        }
        let diff_out = diff_cmd.output().context("failed to execute git diff-index")?;
        anyhow::ensure!(diff_out.status.success(), "failed to check git status on HEAD");
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
            let head_out = std::process::Command::new("git")
                .current_dir(root)
                .args(["rev-parse", "--verify", "HEAD"])
                .output()
                .context("failed to execute git rev-parse HEAD")?;
            anyhow::ensure!(
                head_out.status.success(),
                "cannot create git commit: repository has no commits on HEAD"
            );
            let head_sha = String::from_utf8_lossy(&head_out.stdout).trim().to_string();

            let temp_index_path = git_dir.join(format!(
                "prod_code_prune_index_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));

            struct TempIndexGuard<'a>(&'a std::path::Path);
            impl<'a> Drop for TempIndexGuard<'a> {
                fn drop(&mut self) {
                    let _ = std::fs::remove_file(self.0);
                }
            }
            let _guard = TempIndexGuard(&temp_index_path);

            let mut read_tree = std::process::Command::new("git");
            read_tree
                .current_dir(root)
                .args(["read-tree", "HEAD"])
                .env("GIT_INDEX_FILE", &temp_index_path);
            let read_out = read_tree.output().context("failed to execute git read-tree")?;
            anyhow::ensure!(
                read_out.status.success(),
                "git read-tree failed: {}",
                String::from_utf8_lossy(&read_out.stderr)
            );

            let mut apply_cmd = std::process::Command::new("git");
            apply_cmd
                .current_dir(root)
                .args(["apply", "--cached", "-"])
                .env("GIT_INDEX_FILE", &temp_index_path)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            let mut child = apply_cmd.spawn().context("failed to spawn git apply --cached")?;
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                stdin.write_all(patch.as_bytes()).context("failed to write patch to git apply")?;
            }
            let apply_out = child.wait_with_output().context("failed to wait on git apply")?;
            anyhow::ensure!(
                apply_out.status.success(),
                "git apply --cached failed: {}",
                String::from_utf8_lossy(&apply_out.stderr)
            );

            let mut write_tree = std::process::Command::new("git");
            write_tree
                .current_dir(root)
                .arg("write-tree")
                .env("GIT_INDEX_FILE", &temp_index_path);
            let write_out = write_tree.output().context("failed to execute git write-tree")?;
            anyhow::ensure!(
                write_out.status.success(),
                "git write-tree failed: {}",
                String::from_utf8_lossy(&write_out.stderr)
            );
            let tree_sha = String::from_utf8_lossy(&write_out.stdout).trim().to_string();

            let commit_title = format!("refactor(prune): remove {} unreferenced orphan(s)", pruned.removed.len());
            let mut commit_body = format!(
                "Pruned {} orphan(s) of {} symbol(s) checked:\n",
                pruned.removed.len(),
                pruned.symbols_checked
            );
            for d in &pruned.removed {
                commit_body.push_str(&format!("  - {} {} ({}:{}\n)", d.kind, d.name, d.file, d.line));
            }
            let commit_msg = format!("{commit_title}\n\n{commit_body}");

            let commit_tree_out = std::process::Command::new("git")
                .current_dir(root)
                .args(["commit-tree", &tree_sha, "-p", &head_sha, "-m", &commit_msg])
                .env("GIT_AUTHOR_NAME", "Alexander Panasenko")
                .env("GIT_AUTHOR_EMAIL", "alex@prod.codes")
                .env("GIT_COMMITTER_NAME", "Alexander Panasenko")
                .env("GIT_COMMITTER_EMAIL", "alex@prod.codes")
                .output()
                .context("failed to execute git commit-tree")?;
            anyhow::ensure!(
                commit_tree_out.status.success(),
                "git commit-tree failed: {}",
                String::from_utf8_lossy(&commit_tree_out.stderr)
            );
            let commit_sha = String::from_utf8_lossy(&commit_tree_out.stdout).trim().to_string();

            let update_ref_out = std::process::Command::new("git")
                .current_dir(root)
                .args(["update-ref", "HEAD", &commit_sha, &head_sha])
                .output()
                .context("failed to execute git update-ref")?;
            anyhow::ensure!(
                update_ref_out.status.success(),
                "git update-ref failed: {}",
                String::from_utf8_lossy(&update_ref_out.stderr)
            );

            // Synchronize ambient repository index for the touched files only,
            // leaving any other unrelated staged changes in the ambient index untouched.
            let mut reset_cmd = std::process::Command::new("git");
            reset_cmd.current_dir(root).args(["reset", "HEAD", "--"]);
            for (path, _) in &pruned.rewritten {
                let p = Path::new(path);
                let rel = p.strip_prefix(root).unwrap_or(p);
                reset_cmd.arg(rel);
            }
            let reset_out = reset_cmd.output().context("failed to execute git reset HEAD")?;
            anyhow::ensure!(
                reset_out.status.success(),
                "git reset HEAD failed: {}",
                String::from_utf8_lossy(&reset_out.stderr)
            );

            pruned.git_commit = Some(commit_sha);
        }
    }

    Ok(pruned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(sl: u64, sc: u64, el: u64, ec: u64) -> serde_json::Value {
        serde_json::json!({
            "range": { "start": { "line": sl, "character": sc }, "end": { "line": el, "character": ec } },
            "newText": ""
        })
    }

    #[test]
    fn a_deletion_that_overlaps_another_is_left_for_the_next_run() {
        let mut merged = BTreeMap::new();
        assert!(merge(
            &mut merged,
            vec![("a".into(), vec![edit(0, 0, 3, 0)])]
        ));
        assert!(merge(
            &mut merged,
            vec![("a".into(), vec![edit(3, 0, 5, 0)])]
        ));
        assert!(!merge(
            &mut merged,
            vec![("a".into(), vec![edit(2, 0, 4, 0)])]
        ));
        assert!(merge(
            &mut merged,
            vec![("b".into(), vec![edit(2, 0, 4, 0)])]
        ));
        assert_eq!(merged["a"].len(), 2);
    }

    #[test]
    fn two_whole_file_answers_reduce_to_edits_that_do_not_overlap() {
        let old = "a\nfn one() {}\nb\nfn two() {}\nc\n";
        let first = minimal_edits(old, "a\nb\nfn two() {}\nc\n");
        let second = minimal_edits(old, "a\nfn one() {}\nb\nc\n");
        let mut merged = BTreeMap::new();
        assert!(merge(&mut merged, vec![("f".into(), first)]));
        assert!(merge(&mut merged, vec![("f".into(), second)]));
        assert_eq!(
            crate::refactor::apply_text_edits(old, &merged["f"]).unwrap(),
            "a\nb\nc\n"
        );
    }

    #[test]
    fn only_text_edits_are_taken_from_an_answer() {
        let changes = serde_json::json!({ "changes": { "file:///x.rs": [edit(0, 0, 1, 0)] } });
        assert_eq!(text_edits(&changes).unwrap().len(), 1);
        let doc = serde_json::json!({ "documentChanges": [
            { "textDocument": { "uri": "file:///x.rs", "version": null }, "edits": [edit(0, 0, 1, 0)] }
        ] });
        assert_eq!(text_edits(&doc).unwrap()[0].0, "file:///x.rs");
        let moves = serde_json::json!({ "documentChanges": [ { "kind": "delete", "uri": "file:///x.rs" } ] });
        assert!(text_edits(&moves).is_none());
    }

    #[test]
    fn test_generate_git_patch_format() {
        let pruned = Pruned {
            root: PathBuf::from("/workspace"),
            removed: vec![DeadItem {
                name: "unused_helper".into(),
                kind: "function".into(),
                file: "src/lib.rs".into(),
                line: 12,
                col: 4,
                exported: false,
            }],
            skipped: Vec::new(),
            rewritten: vec![("/workspace/src/lib.rs".into(), "fn active() {}\n".into())],
            diagnostics: Vec::new(),
            applied: false,
            symbols_checked: 10,
            unverified: Vec::new(),
            git_patch: None,
            git_commit: None,
        };

        let patch = pruned.generate_git_patch().unwrap();
        assert!(patch.contains("From: Alexander Panasenko <alex@prod.codes>"));
        assert!(patch.contains("Subject: [PATCH] refactor(prune): remove 1 unreferenced orphan(s)"));
        assert!(patch.contains("Pruned 1 orphan(s) of 10 symbol(s) checked:"));
        assert!(patch.contains("  - function unused_helper (src/lib.rs:12)"));
        assert!(patch.contains("diff --git a/src/lib.rs b/src/lib.rs"));
        assert!(patch.contains("-- \nprod-code\n"));
    }

    #[test]
    fn test_generate_git_patch_empty_when_no_rewrites() {
        let pruned = Pruned {
            root: PathBuf::from("/workspace"),
            removed: Vec::new(),
            skipped: Vec::new(),
            rewritten: Vec::new(),
            diagnostics: Vec::new(),
            applied: false,
            symbols_checked: 5,
            unverified: Vec::new(),
            git_patch: None,
            git_commit: None,
        };
        assert!(pruned.generate_git_patch().is_none());
    }
}
