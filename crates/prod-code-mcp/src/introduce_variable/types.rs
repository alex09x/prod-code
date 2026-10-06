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

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Introduced {
    pub name: String,
    pub expression: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub occurrences: usize,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Introduced {
    pub fn render(&self) -> String {
        let old_text = std::fs::read_to_string(self.root.join(&self.file)).unwrap_or_default();
        let old_text = if self.applied {
            crate::refactor::text_before_apply(&self.root.join(&self.file))
        } else {
            old_text
        };
        let new_text = self
            .rewritten
            .first()
            .map(|(_, t)| t.as_str())
            .unwrap_or("");
        let diff = similar::TextDiff::from_lines(old_text.as_str(), new_text)
            .unified_diff()
            .context_radius(1)
            .header(&format!("a/{}", self.file), &format!("b/{}", self.file))
            .to_string();
        let mut out = format!(
            "`let {} = {};` ({})\n\n- {} occurrence(s) now read `{}`\n\n{diff}",
            self.name, self.expression, self.file, self.occurrences, self.name
        );
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

pub(crate) fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
