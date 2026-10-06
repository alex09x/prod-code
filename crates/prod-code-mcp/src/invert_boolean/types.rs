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

/// What the inversion did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Inverted {
    pub was: String,
    pub now: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    /// What was inverted: `function`, `field` or `variable`.
    pub kind: String,
    /// Calls (or reads) that gained a `!`.
    pub negated: usize,
    /// Calls (or reads) whose `!` was removed, because it and the inversion cancel.
    pub cancelled: usize,
    /// Writes that now store the negation of what they stored: an assignment, a `let` initialiser,
    /// a field in a struct literal.
    pub writes: usize,
    /// Uses that cannot keep their meaning under the inversion; nothing is written while any
    /// remains, unless `force`.
    pub blocked: Vec<String>,
    /// References that were not negated; nothing is written while any remains, forced or not.
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Inverted {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = if self.kind == "function" {
            format!(
                "`{}` → `{}` ({})\n\n- the body returns the negation of what it returned\n- {} \
                 call(s) gain a `!`, {} lose the `!` they had\n\n",
                self.was, self.now, self.file, self.negated, self.cancelled
            )
        } else {
            format!(
                "`{}` → `{}` ({}, a boolean {})\n\n- {} read(s) gain a `!`, {} lose the `!` they \
                 had\n- {} write(s) now store the negation of what they stored\n\n",
                self.was, self.now, self.file, self.kind, self.negated, self.cancelled, self.writes
            )
        };
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
        if !self.blocked.is_empty() {
            out.push_str(&format!(
                "\n{} use(s) cannot keep their meaning under the inversion; nothing is written \
                 while any remains:\n",
                self.blocked.len()
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) that are not a call — a function used as a value \
                 keeps its old meaning under its new name; nothing is written while any remains):\n",
                self.unmatched.len()
            ));
            for r in &self.unmatched {
                out.push_str(&format!("  {r}\n"));
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

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
