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

use super::syntax::display;

/// What the move did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MovedMethod {
    pub method: String,
    pub from_type: String,
    pub to_type: String,
    #[serde(skip)]
    pub root: PathBuf,
    /// The signature the method has now.
    pub signature: String,
    pub calls: usize,
    /// Why nothing may be written unless `force`: a call whose receiver or argument does
    /// something, and would be evaluated in the other order.
    pub blocked: Vec<String>,
    /// References that were not rewritten — the method used as a value, a call this could not
    /// read, a position the file does not match. Nothing is written while any remains, forced or
    /// not: each would still name the method where it was (#446).
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl MovedMethod {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}::{}` is now `{}::{}`: `{}`; {} call(s) rewritten\n",
            self.from_type, self.method, self.to_type, self.method, self.signature, self.calls
        );
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
            if self.diagnostics.iter().any(|d| d.contains("cannot find")) {
                out.push_str(&format!(
                    "\na name the body uses resolves where `{}` was and not where it goes: import \
                     it in the new file, or spell its path, and run this again\n",
                    self.method
                ));
            }
        }
        out.push_str(if self.applied {
            "\n[applied]\n"
        } else {
            "\nnothing was written; pass `apply: true` to make these edits\n"
        });
        out
    }
}
