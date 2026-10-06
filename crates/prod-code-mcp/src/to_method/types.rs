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

use super::helpers::display;

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MadeMethod {
    pub owner: String,
    pub method: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    /// The first parameter as it was declared: `c: &mut Counter`.
    pub parameter: String,
    /// The receiver it became: `&mut self`.
    pub receiver: String,
    /// Uses of the parameter in the body, now `self`.
    pub renamed_uses: usize,
    pub rewritten_calls: usize,
    /// References left as they are, and why: still valid, but not rewritten.
    pub unchanged: Vec<String>,
    /// Positions the analyzer reported where the file does not name the function: what is there
    /// is unknown, so nothing is written while any remains, forced or not (#446).
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl MadeMethod {
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
        let receiver_desc = if self.file.ends_with(".ts")
            || self.file.ends_with(".tsx")
            || self.file.ends_with(".js")
            || self.file.ends_with(".jsx")
            || self.file.ends_with(".cpp")
            || self.file.ends_with(".cc")
            || self.file.ends_with(".cxx")
            || self.file.ends_with(".h")
            || self.file.ends_with(".hpp")
        {
            "this"
        } else if self.file.ends_with(".go") {
            &self.receiver
        } else {
            "self"
        };
        let mut out = format!(
            "`{call_target}` ({})\n\n- the parameter `{}` becomes the receiver `{}`; {} use(s) of it in \
             the body are now `{receiver_desc}`\n- {} call site(s) now call it as a method\n\n",
            self.file, self.parameter, self.receiver, self.renamed_uses, self.rewritten_calls
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
        if !self.unchanged.is_empty() {
            out.push_str(&format!(
                "\nleft as they are, and still valid ({}):\n",
                self.unchanged.len()
            ));
            for r in &self.unchanged {
                out.push_str(&format!("  {r}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str("\nnot rewritten; nothing is written while any remains, forced or not:\n");
            for u in &self.unmatched {
                out.push_str(&format!("  {u}\n"));
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
