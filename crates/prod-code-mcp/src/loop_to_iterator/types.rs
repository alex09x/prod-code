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

/// What the loop builds.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Shape {
    /// `acc += X`, maybe under `if C`.
    Sum { cond: Option<String>, value: String },
    /// `if C { acc += 1 }` into a `usize`.
    Count { cond: String },
    /// `acc.push(X)`, maybe under `if C`.
    Collect { cond: Option<String>, value: String },
    /// `if C { acc = Some(X); break; }` into `Option<T>`.
    Find { cond: String, value: String },
    /// `if C { acc = true; break; }` into `bool`.
    Any { cond: String },
    /// `if !C { acc = false; break; }` into `bool`.
    All { cond: String },
}

/// A recognised polyglot loop replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolyglotLoop {
    pub start: usize,
    pub end: usize,
    pub indent: String,
    pub replacement: String,
    pub statement: String,
}

/// A recognised loop and the statement that declares its accumulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccumulatorLoop {
    /// Where the `let` line starts and where the loop's closing brace ends.
    pub start: usize,
    pub end: usize,
    pub indent: String,
    pub acc: String,
    /// The declared type, when the `let` has one.
    pub declared: Option<String>,
    pub pattern: String,
    pub source: String,
    pub shape: Shape,
}

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Rewritten {
    pub statement: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub new_text: String,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Rewritten {
    pub fn render(&self) -> String {
        let full = self.root.join(&self.file);
        let old_text = if self.applied {
            crate::refactor::text_before_apply(&full)
        } else {
            std::fs::read_to_string(&full).unwrap_or_default()
        };
        let mut out = format!(
            "{}\n\n{}",
            self.statement.trim(),
            similar::TextDiff::from_lines(old_text.as_str(), self.new_text.as_str())
                .unified_diff()
                .context_radius(1)
                .header(&format!("a/{}", self.file), &format!("b/{}", self.file))
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
