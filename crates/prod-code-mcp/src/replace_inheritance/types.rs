/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;

/// Outcome of replacing inheritance with delegation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReplaceInheritanceResult {
    pub sub_type: String,
    pub base_type: String,
    pub field_name: String,
    pub forwarded_methods: Vec<String>,
    pub files_modified: Vec<String>,
    #[serde(skip)]
    pub overlays: Vec<(PathBuf, String)>,
    pub diff: String,
    pub applied: bool,
    pub verified: bool,
    pub diagnostics: Vec<String>,
}

impl ReplaceInheritanceResult {
    pub fn render(&self, max_diff_len: usize) -> String {
        let mut out = format!(
            "`{}` — replaced inheritance from `{}` with delegation via `{}`\n",
            self.sub_type, self.base_type, self.field_name
        );
        out.push_str(&format!(
            "- forwarded methods ({}): {}\n",
            self.forwarded_methods.len(),
            if self.forwarded_methods.is_empty() {
                "none".to_string()
            } else {
                self.forwarded_methods.join(", ")
            }
        ));
        out.push_str(&format!(
            "- files modified ({}): {}\n",
            self.files_modified.len(),
            self.files_modified.join(", ")
        ));
        out.push_str(&format!("- applied: {}\n", self.applied));
        out.push_str(&format!("- verified: {}\n", self.verified));
        if !self.diagnostics.is_empty() {
            out.push_str(&format!("- diagnostics ({}):\n", self.diagnostics.len()));
            for d in &self.diagnostics {
                out.push_str(&format!("  • {d}\n"));
            }
        }
        if !self.diff.is_empty() {
            out.push_str("\nDiff:\n```diff\n");
            if self.diff.len() > max_diff_len {
                out.push_str(&self.diff[..max_diff_len]);
                out.push_str("\n... [truncated]\n");
            } else {
                out.push_str(&self.diff);
            }
            out.push_str("```\n");
        }
        out
    }
}
