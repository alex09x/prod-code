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

use crate::make_static::helpers::display;

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MadeStatic {
    pub owner: String,
    pub method: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    /// The receiver the declaration had: `&self`, `&mut self`, `self`.
    pub receiver: String,
    pub rewritten_calls: usize,
    /// Call sites that cannot be safely rewritten, due to receiver effects or unknown type.
    pub blocked: Vec<String>,
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl MadeStatic {
    pub fn render(&self, diff_budget: usize) -> String {
        let sep = if self.file.ends_with(".ts")
            || self.file.ends_with(".tsx")
            || self.file.ends_with(".js")
            || self.file.ends_with(".jsx")
            || self.file.ends_with(".py")
            || self.file.ends_with(".swift")
        {
            "."
        } else if self.file.ends_with(".go") {
            if self.owner.is_empty() { "" } else { "." }
        } else {
            "::"
        };
        let call_target = if self.owner.is_empty() {
            self.method.clone()
        } else {
            format!("{}{sep}{}", self.owner, self.method)
        };
        let mut out = format!(
            "`{call_target}` ({})\n\n- the receiver `{}` is removed: it was never used\n- {} call site(s) \
             now call `{call_target}`\n\n",
            self.file, self.receiver, self.rewritten_calls,
        );
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
                "\n{} call site(s) would drop a receiver that does something when it is evaluated; \
                 bind it to a variable first, or keep the method:\n",
                self.blocked.len()
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) this could not read as a call):\n",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    TypeScript,
    Python,
    Cpp,
    Swift,
    Go,
}

impl Language {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension().and_then(|s| s.to_str()) {
            Some("ts" | "tsx" | "js" | "jsx") => Some(Self::TypeScript),
            Some("py") => Some(Self::Python),
            Some("cpp" | "cc" | "cxx" | "h" | "hpp") => Some(Self::Cpp),
            Some("swift") => Some(Self::Swift),
            Some("go") => Some(Self::Go),
            _ => None,
        }
    }

    pub fn matches_extension(&self, path: &Path) -> bool {
        Self::from_path(path) == Some(*self)
    }
}
