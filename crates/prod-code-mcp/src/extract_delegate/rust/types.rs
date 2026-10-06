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

/// One field of a struct: its text from its first attribute or doc line to its comma.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub vis: String,
    /// The field's text without the trailing comma, first line trimmed of its indentation.
    pub text: String,
}

/// A struct declaration read from the source.
#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub vis: String,
    pub start: usize,
    pub open: usize,
    pub close: usize,
    pub derive: Option<String>,
    pub fields: Vec<Field>,
}

/// What the extraction did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Extracted {
    pub helper: String,
    pub field: String,
    pub fields: Vec<String>,
    pub methods: Vec<String>,
    #[serde(skip)]
    pub root: PathBuf,
    pub rewritten: Vec<(String, String)>,
    pub accesses: usize,
    pub unmatched: Vec<String>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Extracted {
    pub fn render(&self) -> String {
        let mut out = format!(
            "`{}: {}` holds {}; {} method(s) moved, {} access(es) rerouted\n",
            self.field,
            self.helper,
            self.fields.join(", "),
            self.methods.len(),
            self.accesses
        );
        for (path, new_text) in &self.rewritten {
            let full = Path::new(path);
            let old_text = if self.applied {
                crate::refactor::text_before_apply(full)
            } else {
                std::fs::read_to_string(full).unwrap_or_default()
            };
            let rel = full
                .strip_prefix(&self.root)
                .unwrap_or(full)
                .display()
                .to_string();
            out.push('\n');
            out.push_str(
                &similar::TextDiff::from_lines(old_text.as_str(), new_text.as_str())
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        if !self.unmatched.is_empty() {
            out.push_str("\nunresolved references (the edit is incomplete):\n");
            for reference in &self.unmatched {
                out.push_str(&format!("  • {reference}\n"));
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
        out.push_str(if self.applied {
            "\n[applied]\n"
        } else {
            "\nnothing was written; pass `apply: true` to make this edit\n"
        });
        out
    }
}
