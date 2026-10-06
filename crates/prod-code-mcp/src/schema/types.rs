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

use super::edits::make_workspace_edit_with_ends;
use super::scan::display;

/// One spelling of the field, and what it becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub from: String,
    pub to: String,
    pub style: &'static str,
}

/// Where a spelling appears. Positions are 1-based, `col` and `len` counted in characters.
#[derive(Debug, Clone)]
pub(crate) struct Occurrence {
    pub(crate) file: PathBuf,
    pub(crate) line: u32,
    pub(crate) col: u32,
    pub(crate) len: usize,
    pub(crate) variant: usize,
    pub(crate) in_string: bool,
}

/// Returns the exact LSP 0-based (line, character) end position of `text`.
/// Character offset is counted in UTF-16 code units per the LSP specification.
pub fn lsp_end_position(text: &str) -> (u32, u32) {
    let mut line = 0u32;
    let mut col_utf16 = 0u32;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            col_utf16 = 0;
        } else if ch != '\r' {
            col_utf16 += ch.len_utf16() as u32;
        }
    }
    (line, col_utf16)
}

/// What the rename did, or would do.
#[derive(Debug)]
pub struct SchemaRename {
    pub field: String,
    pub to: String,
    pub root: PathBuf,
    /// Every file this changes, as (path, whole new content).
    pub rewritten: Vec<(PathBuf, String)>,
    /// Line count of each file before the rename was applied.
    pub original_lines: BTreeMap<PathBuf, usize>,
    /// LSP (line, character) end position of each file before the rename was applied.
    pub original_ends: BTreeMap<PathBuf, (u32, u32)>,
    /// One line per language: how many occurrences, and how they were handled.
    pub summary: Vec<String>,
    /// Occurrences nothing rewrote: a comment, a language with no engine here, a rename the
    /// analyzer refused.
    pub left: Vec<String>,
    /// Errors the analyzers report for the changed files, checked per project.
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl SchemaRename {
    /// Generates an atomic LSP `WorkspaceEdit` (`documentChanges`) for this schema rename.
    pub fn workspace_edit(&self) -> serde_json::Value {
        make_workspace_edit_with_ends(&self.rewritten, &self.original_ends)
    }

    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!("`{}` → `{}`\n\n", self.field, self.to);
        for line in &self.summary {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
        let mut body = String::new();
        let mut changed = 0usize;
        for (path, new_text) in &self.rewritten {
            let old = crate::refactor::text_before_apply(Path::new(path));
            let rel = display(&self.root, path);
            let diff = similar::TextDiff::from_lines(&old, new_text);
            changed += diff
                .iter_all_changes()
                .filter(|c| c.tag() != similar::ChangeTag::Equal)
                .count();
            body.push_str(
                &diff
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        out.push_str(&format!(
            "{} changed line(s) in {} file(s)\n\n",
            changed,
            self.rewritten.len()
        ));
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }
        if !self.left.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({}), because no analyzer owns them and they are not string \
                 literals — usually comments and documentation:\n",
                self.left.len()
            ));
            for l in &self.left {
                out.push_str(&format!("  {l}\n"));
            }
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzers accept the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzers reject the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        if self.applied {
            out.push_str(&format!(
                "\n[applied to {} file(s)]\n",
                self.rewritten.len()
            ));
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make these edits\n");
        }
        out
    }
}

/// A rename across several repositories: the backend that owns the schema and the frontends
/// and services that read it.
#[derive(Debug)]
pub struct AcrossRepos {
    /// One plan per repository the field appears in, in the order given.
    pub repos: Vec<SchemaRename>,
    /// The repositories it does not appear in.
    pub missing: Vec<PathBuf>,
    pub applied: bool,
}

impl AcrossRepos {
    /// Generates an atomic multi-repository LSP `WorkspaceEdit` (`documentChanges`) across all repositories.
    pub fn workspace_edit(&self) -> serde_json::Value {
        let mut all_ends = BTreeMap::new();
        let mut all_rewritten = Vec::new();
        for repo in &self.repos {
            all_ends.extend(repo.original_ends.clone());
            all_rewritten.extend(repo.rewritten.clone());
        }
        make_workspace_edit_with_ends(&all_rewritten, &all_ends)
    }

    /// Every repository's analyzers accept its result.
    pub fn clean(&self) -> bool {
        self.repos.iter().all(|r| r.diagnostics.is_empty())
    }

    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = String::new();
        let share = diff_budget / self.repos.len().max(1);
        for repo in &self.repos {
            out.push_str(&format!(
                "## {}\n\n{}\n",
                repo.root.display(),
                repo.render(share)
            ));
        }
        for root in &self.missing {
            out.push_str(&format!(
                "## {}\n\nthe field does not appear here\n\n",
                root.display()
            ));
        }
        out.push_str(&if self.applied {
            format!("[applied to {} repositories together]\n", self.repos.len())
        } else {
            format!(
                "nothing was written in any of the {} repositories; `apply: true` writes them all \
                 or none\n",
                self.repos.len() + self.missing.len()
            )
        });
        out
    }
}
