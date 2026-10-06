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

/// What the inlining did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InlinedParameter {
    pub function: String,
    pub parameter: String,
    /// The value every call passed, now bound at the top of the body.
    pub value: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub rewritten_calls: usize,
    /// References that are not a call with this parameter's argument: the function used as a
    /// value, or a call this could not read. They block the write.
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl InlinedParameter {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` of `{}` ({})\n\n- every call passes `{}`: it is bound at the top of the body\n- \
             {} call(s) lose the argument\n\n",
            self.parameter, self.function, self.file, self.value, self.rewritten_calls
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
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot a call with this argument ({}); nothing is written while any remains:\n",
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

pub struct PolyglotDecl {
    pub fn_name: String,
    pub open_paren: usize,
    pub close_paren: usize,
    pub body_open: usize,
    pub body_close: usize,
    pub receiver: Option<String>,
    pub params: Vec<crate::parameter_object::Param>,
}

pub struct FoundCall {
    pub args_start: usize,
    pub args_end: usize,
    pub arg_index: usize,
    pub passed_value: String,
    pub site: String,
}
