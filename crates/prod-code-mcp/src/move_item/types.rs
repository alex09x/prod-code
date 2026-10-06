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

use super::item::display;

/// What a move did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Move {
    pub symbol: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub from: String,
    pub from_module: String,
    pub to: String,
    pub to_module: String,
    /// The path a file outside the target module imports now, e.g. `prod_code_mcp::fixture::Shape`.
    pub new_path: String,
    pub moved_lines: usize,
    /// Every file this touched, whole, ready to be written.
    pub rewritten: Vec<(String, String)>,
    /// One line per import that was added, rewritten or dropped.
    pub imports: Vec<String>,
    /// References the rewrite did not understand, named rather than guessed at.
    pub left_alone: Vec<String>,
    /// Positions the analyzer reported where the file does not name the item: not rewritten,
    /// they would still reach it where it was. Nothing is written while any remains, forced or
    /// not (#446).
    pub unmatched: Vec<String>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
    /// The target module did not exist: the file created, and the parent that declares it now.
    pub created: Option<(String, String)>,
}

impl Move {
    /// The report: what moved, what it costs the files that used it, and whether it compiles.
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` moved\n\n- from: {} ({})\n- to:   {} ({})\n- callers now import: `{}`\n- {} \
             line(s) moved\n\n",
            self.symbol,
            self.from,
            self.from_module,
            self.to,
            self.to_module,
            self.new_path,
            self.moved_lines
        );
        if let Some((file, parent)) = &self.created {
            out.insert_str(
                out.len() - 1,
                &format!("- {file} is new, declared in {parent}\n"),
            );
        }
        let mut body = String::new();
        let mut changed_lines = 0usize;
        for (path, new_text) in &self.rewritten {
            let old_text = crate::refactor::text_before_apply(Path::new(path));
            let rel = display(&self.root, Path::new(path));
            let diff = similar::TextDiff::from_lines(&old_text, new_text);
            changed_lines += diff
                .iter_all_changes()
                .filter(|c| c.tag() != similar::ChangeTag::Equal)
                .count();
            body.push_str(
                &diff
                    .unified_diff()
                    .context_radius(2)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        out.push_str(&format!(
            "{} changed line(s) in {} file(s)\n\n",
            changed_lines,
            self.rewritten.len()
        ));
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }
        if !self.imports.is_empty() {
            out.push_str("\nimports:\n");
            for i in &self.imports {
                out.push_str(&format!("  {i}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str("\nnot rewritten; nothing is written while any remains, forced or not:\n");
            for u in &self.unmatched {
                out.push_str(&format!("  {u}\n"));
            }
        }
        if !self.left_alone.is_empty() {
            out.push_str("\nleft alone:\n");
            for l in &self.left_alone {
                out.push_str(&format!("  {l}\n"));
            }
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
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

/// Where a file sits in its crate, in the spelling a `use` needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModulePath {
    /// The crate's name as Rust spells it: dashes turned into underscores.
    pub krate: String,
    /// The module segments under the crate root; empty for the root itself.
    pub segments: Vec<String>,
}

impl ModulePath {
    /// How a file belonging to `from_crate` spells this module: `crate::a::b` inside the same
    /// crate, `the_crate::a::b` from outside it.
    pub fn spelled_from(&self, from_crate: &str) -> String {
        let head = if from_crate == self.krate {
            "crate".to_string()
        } else {
            self.krate.clone()
        };
        std::iter::once(head)
            .chain(self.segments.iter().cloned())
            .collect::<Vec<_>>()
            .join("::")
    }

    /// The unambiguous spelling, for a report: always the crate's own name.
    pub fn absolute(&self) -> String {
        std::iter::once(self.krate.clone())
            .chain(self.segments.iter().cloned())
            .collect::<Vec<_>>()
            .join("::")
    }
}
