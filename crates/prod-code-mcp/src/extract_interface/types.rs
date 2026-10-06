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

/// Result of extracting an interface.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractInterfaceResult {
    pub type_name: String,
    pub interface_name: String,
    pub methods: Vec<String>,
    pub files_modified: Vec<String>,
    #[serde(skip)]
    pub overlays: Vec<(PathBuf, String)>,
    pub diff: String,
    pub applied: bool,
    pub verified: bool,
    pub diagnostics: Vec<String>,
}

impl ExtractInterfaceResult {
    pub fn render(&self, max_diff_len: usize) -> String {
        let mut out = format!(
            "`interface {}` for `{}`\n- extracted methods: {}\n- applied: {}\n",
            self.interface_name,
            self.type_name,
            if self.methods.is_empty() {
                "none".to_string()
            } else {
                self.methods.join(", ")
            },
            self.applied,
        );
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
