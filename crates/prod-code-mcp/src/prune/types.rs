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
        out.push('\n');
        let mut body = String::new();
        for (path, new_text) in &self.rewritten {
            let full = Path::new(path);
            let old = if self.applied {
                crate::refactor::text_before_apply(full)
            } else {
                std::fs::read_to_string(full).unwrap_or_default()
            };
            let rel = full
                .strip_prefix(&self.root)
                .unwrap_or(full)
                .to_string_lossy();
            body.push_str(
                &similar::TextDiff::from_lines(&old, new_text)
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        if body.len() > 10_000 {
            let cut: String = body.chars().take(10_000).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        out.push_str(if self.applied {
            "\n[applied]\n"
        } else {
            "\nnothing was written (pass `apply` to prune)\n"
        });

        if let Some(commit) = &self.git_commit {
            out.push_str(&format!("\nCreated Git commit: {commit}\n"));
        } else if let Some(patch) = &self.git_patch {
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
            patch.push_str(&format!(
                "  - {} {} ({}:{})\n",
                d.kind, d.name, d.file, d.line
            ));
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
            patch.push_str(&format!(
                " {:<35} | {:>4} {plus_bar}{minus_bar}\n",
                rel, count
            ));
        }
        let file_s = if file_diffs.len() == 1 {
            "file"
        } else {
            "files"
        };
        let ins_s = if total_added == 1 {
            "insertion"
        } else {
            "insertions"
        };
        let del_s = if total_deleted == 1 {
            "deletion"
        } else {
            "deletions"
        };
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
