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

/// Where a method is declared: in a trait, or in an implementation of one.
#[derive(Debug, Clone, PartialEq)]
pub enum Owner {
    /// In `trait Name { … }`.
    Trait { name: String },
    /// In `impl Trait for Type { … }`; the offset of the trait's name in the header.
    Impl { trait_at: usize },
}

/// What removing the parameter did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TraitParameter {
    pub method: String,
    pub parameter: String,
    /// The parameter's position, not counting the receiver.
    pub index: usize,
    #[serde(skip)]
    pub root: PathBuf,
    /// `file:line` of the trait's declaration and of every implementation.
    pub declarations: Vec<String>,
    pub calls: usize,
    /// Why nothing may be written unless `force`: a body that uses the parameter, an argument
    /// that does something.
    pub blocked: Vec<String>,
    /// What was not rewritten — a use that is not a call, a call or declaration this could not
    /// read, a position the file does not match. Nothing is written while any remains, forced or
    /// not: each would still pass or declare the parameter (#446).
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl TraitParameter {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "the parameter `{}` (#{} after the receiver) leaves `{}`: {} declaration(s), {} call(s)\n",
            self.parameter,
            self.index + 1,
            self.method,
            self.declarations.len(),
            self.calls
        );
        for d in &self.declarations {
            out.push_str(&format!("- {d}\n"));
        }
        if !self.blocked.is_empty() {
            out.push_str("\nnothing may be written while:\n");
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
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
            let old = crate::refactor::text_before_apply(Path::new(path));
            let rel = display(&self.root, Path::new(path));
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
            "\nnothing was written\n"
        });
        out
    }
}

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
