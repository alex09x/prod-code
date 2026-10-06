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
use std::path::{Path, PathBuf};

use anyhow::Result;

/// What a module move did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModuleMove {
    pub module: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub from_module: String,
    pub to_module: String,
    /// Old and new path of every file that moves, relative to the checkout.
    pub moved: Vec<(String, String)>,
    /// Every file this writes, whole: the moved files at their new paths and the files whose
    /// paths or declarations changed.
    pub rewritten: Vec<(String, String)>,
    /// The files that go away: the moved files at their old paths.
    pub removed: Vec<String>,
    /// One line per path or import that was spelled anew.
    pub notes: Vec<String>,
    /// Paths the analyzer reported where the file does not name the module: not rewritten, they
    /// would still name it where it was. Nothing is written while any remains, forced or not.
    pub unmatched: Vec<String>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl ModuleMove {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "module `{}` moved: {} -> {}\n\n",
            self.module, self.from_module, self.to_module
        );
        for (from, to) in &self.moved {
            out.push_str(&format!("- {from} -> {to}\n"));
        }
        if !self.notes.is_empty() {
            out.push('\n');
            for n in &self.notes {
                out.push_str(&format!("- {n}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str("\nnot rewritten; nothing is written while any remains, forced or not:\n");
            for u in &self.unmatched {
                out.push_str(&format!("  {u}\n"));
            }
        }
        out.push('\n');
        let mut body = String::new();
        for (path, new_text) in &self.rewritten {
            let p = Path::new(path);
            let old = if self.removed.iter().any(|r| Path::new(r) == p) {
                // A file that moves: its diff is shown as new, its old path is recorded as removed.
                String::new()
            } else {
                crate::refactor::text_before_apply(p)
            };
            let rel = display(&self.root, p);
            body.push_str(
                &similar::TextDiff::from_lines(&old, new_text)
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
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
            "\nnothing was written (call `write` to apply)\n"
        });
        out
    }

    /// Writes the move: the new files, the rewritten ones, and the old files removed. Refused
    /// while the analyzer rejects the result, unless `force`, and while a path was not rewritten,
    /// forced or not (#446).
    pub fn write(&mut self, force: bool) -> Result<()> {
        anyhow::ensure!(
            self.unmatched.is_empty(),
            "{} path(s) to `{}` were not rewritten; nothing was written:\n  {}",
            self.unmatched.len(),
            self.module,
            self.unmatched.join("\n  ")
        );
        anyhow::ensure!(
            self.diagnostics.is_empty() || force,
            "the move does not compile ({} error(s)); nothing was written:\n  {}",
            self.diagnostics.len(),
            self.diagnostics.join("\n  ")
        );
        let files: BTreeMap<PathBuf, String> = self
            .rewritten
            .iter()
            .map(|(p, t)| (PathBuf::from(p), t.clone()))
            .collect();
        let mut edit = crate::signature::whole_file_edit(&files);
        if let Some(changes) = edit
            .get_mut("documentChanges")
            .and_then(|c| c.as_array_mut())
        {
            for old in &self.removed {
                changes.push(serde_json::json!({
                    "kind": "delete",
                    "uri": prod_code_protocol::path::file_uri(&self.root.join(old)),
                }));
            }
        }
        crate::refactor::apply_workspace_edit(&self.root, &edit)?;
        // The directories the module left, when nothing else is in them.
        let mut dirs: Vec<PathBuf> = self
            .removed
            .iter()
            .filter_map(|old| self.root.join(old).parent().map(Path::to_path_buf))
            .collect();
        dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        dirs.dedup();
        for dir in dirs {
            let _ = std::fs::remove_dir(dir);
        }
        self.applied = true;
        Ok(())
    }
}

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
