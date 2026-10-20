/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Literal and regex text search (`code_grep` / `prod-code grep`): runs ripgrep (`rg --json`) on
//! the gateway node over the mirrored checkout (with gitignore-aware `ignore::WalkBuilder` +
//! `OverrideBuilder` fallback), attaching the enclosing symbol (function/method/type) to each hit
//! so agents can immediately follow up with `code_definition` or `code_references` (#1119).

pub mod enclosing;
pub mod execute;
pub(crate) mod fallback;

#[cfg(test)]
mod tests;

pub use enclosing::{find_enclosing_symbol_in_file, find_enclosing_symbol_in_lines};
pub use execute::run_grep;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default maximum number of matches returned by `code_grep` when `max_results` is omitted.
pub const DEFAULT_MAX_RESULTS: usize = 50;
/// Hard ceiling on `max_results` to keep agent context windows bounded.
pub const MAX_RESULTS_CAP: usize = 500;
/// Hard ceiling on `context` lines before/after each match.
pub const MAX_CONTEXT_LINES: usize = 10;

/// Options for [`run_grep`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepOptions {
    /// Regex or literal pattern to search for.
    pub pattern: String,
    /// Optional file or directory path to restrict the search scope.
    pub path: Option<PathBuf>,
    /// Treat `pattern` as a literal string (`-F` / `--fixed-strings`) instead of a regular expression.
    pub fixed_strings: bool,
    /// Case-insensitive matching (`-i` / `--ignore-case`). When false, uses smart-case (`-S`):
    /// case-insensitive if the pattern is all lowercase, case-sensitive if it contains uppercase.
    pub ignore_case: bool,
    /// Optional include file globs (e.g. `["*.rs", "*.go"]`).
    pub include: Vec<String>,
    /// Optional exclude file globs (e.g. `["*_test.go", "fixtures/**"]`).
    pub exclude: Vec<String>,
    /// Maximum number of matches to return (default 50, capped at 500).
    pub max_results: usize,
    /// Number of context lines before and after each match (default 0, capped at 10).
    pub context: usize,
}

impl Default for GrepOptions {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            path: None,
            fixed_strings: false,
            ignore_case: false,
            include: Vec::new(),
            exclude: Vec::new(),
            max_results: DEFAULT_MAX_RESULTS,
            context: 0,
        }
    }
}

/// One matched line returned by `code_grep`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GrepHit {
    /// Path relative to the workspace root (or absolute when outside the workspace).
    pub path: String,
    /// 1-based line number of the match.
    pub line: u32,
    /// 1-based column number of the first match on the line.
    pub column: u32,
    /// Matched line text (trimmed of trailing newline).
    pub text: String,
    /// Enclosing function, method, or type declaration (e.g. `Type::method`, `fn name`), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enclosing_symbol: Option<String>,
    /// Context lines immediately preceding the match: `(line_number, text)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_before: Vec<(u32, String)>,
    /// Context lines immediately following the match: `(line_number, text)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_after: Vec<(u32, String)>,
}

/// Complete result of a `code_grep` invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepReport {
    pub pattern: String,
    pub hits: Vec<GrepHit>,
    pub total_matches: usize,
    pub truncated: bool,
    pub engine: String,
}

impl GrepReport {
    /// Renders a human- and agent-friendly summary with `file:line:col`, `[in Symbol]`, and snippet.
    pub fn render(&self) -> String {
        if self.hits.is_empty() {
            return format!("No matches found for `{}`.", self.pattern);
        }
        let mut out = String::new();
        if self.truncated {
            out.push_str(&format!(
                "Found {}+ match(es) for `{}` (showing first {}):\n",
                self.total_matches,
                self.pattern,
                self.hits.len()
            ));
        } else {
            out.push_str(&format!(
                "Found {} match(es) for `{}`:\n",
                self.hits.len(),
                self.pattern
            ));
        }
        for hit in &self.hits {
            let enclosing = match &hit.enclosing_symbol {
                Some(sym) => format!("  [in {sym}]"),
                None => String::new(),
            };
            out.push_str(&format!(
                "  • {}:{}:{}{} — {}\n",
                hit.path,
                hit.line,
                hit.column,
                enclosing,
                hit.text.trim()
            ));
            for (ctx_line, ctx_text) in &hit.context_before {
                out.push_str(&format!("      {ctx_line:>5} | {ctx_text}\n"));
            }
            if !hit.context_before.is_empty() || !hit.context_after.is_empty() {
                out.push_str(&format!("    > {:>5} | {}\n", hit.line, hit.text));
            }
            for (ctx_line, ctx_text) in &hit.context_after {
                out.push_str(&format!("      {ctx_line:>5} | {ctx_text}\n"));
            }
        }
        if self.truncated {
            out.push_str(
                "\n(Result truncated at max_results; pass `path`, `include`/`exclude` globs, or a higher `max_results` to narrow or expand.)",
            );
        }
        out.trim_end().to_string()
    }
}
