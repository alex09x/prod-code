/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HallucinationKind {
    InvalidMethodInvocation,
    IncorrectArgumentType,
    BorrowCheckerError,
    SyntaxError,
    UnresolvedIdentifier,
}

impl std::fmt::Display for HallucinationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMethodInvocation => write!(f, "InvalidMethodInvocation"),
            Self::IncorrectArgumentType => write!(f, "IncorrectArgumentType"),
            Self::BorrowCheckerError => write!(f, "BorrowCheckerError"),
            Self::SyntaxError => write!(f, "SyntaxError"),
            Self::UnresolvedIdentifier => write!(f, "UnresolvedIdentifier"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HallucinationInterception {
    pub kind: HallucinationKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_or_target: Option<String>,
    pub message: String,
    pub line: u32,
    pub col: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// Result of validating a streamed sequence of chunks during agent code generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamValidationResult {
    pub completed_chunks: usize,
    pub total_chunks: usize,
    pub intercepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interception: Option<HallucinationInterception>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intercept_chunk_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_report: Option<DiagnosticsReport>,
    pub summary: String,
}

impl StreamValidationResult {
    pub fn render(&self) -> String {
        let mut out = format!("Streamed validation: {}\n", self.summary);
        if let Some(intercept) = &self.interception {
            out.push_str(&format!(
                "  [INTERCEPT at chunk {}]: {}\n",
                self.intercept_chunk_index.map(|i| i + 1).unwrap_or(0),
                intercept.message
            ));
            if let Some(sugg) = &intercept.suggestion {
                out.push_str(&format!("    --> {sugg}\n"));
            }
        }
        if let Some(report) = &self.final_report {
            out.push_str(&report.render());
        }
        out
    }
}

/// Result of incrementally validating an incoming chunk in a stateful streaming session (Phase 7.7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunkResult {
    pub session_id: String,
    pub chunk_index: usize,
    pub accumulated_bytes: usize,
    pub checkpoint_evaluated: bool,
    pub intercepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interception: Option<HallucinationInterception>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_report: Option<DiagnosticsReport>,
    pub summary: String,
}

impl StreamChunkResult {
    pub fn render(&self) -> String {
        let mut out = format!(
            "Stream chunk {} [{}]: {}\n",
            self.chunk_index, self.session_id, self.summary
        );
        if let Some(intercept) = &self.interception {
            out.push_str(&format!(
                "  [INTERCEPT at chunk {}]: {}\n",
                self.chunk_index, intercept.message
            ));
            if let Some(sugg) = &intercept.suggestion {
                out.push_str(&format!("    --> {sugg}\n"));
            }
        }
        if let Some(report) = &self.final_report {
            out.push_str(&report.render());
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocDiagnostic {
    pub severity: String,
    pub code: Option<String>,
    pub message: String,
    pub line: u32,
    pub col: u32,
    pub source: Option<String>,
    /// Extra explanation added by prod-code (for example that the failing line uses a symbol
    /// the proposed edits removed or renamed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Where the range ends, 1-based line and column, when the analyzer gave one.
    #[serde(skip, default)]
    pub end: Option<(u32, u32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsReport {
    pub file: String,
    pub errors: usize,
    pub warnings: usize,
    pub items: Vec<DocDiagnostic>,
    /// Diagnostics the file already had on disk, before the edit under review: the same
    /// severity, code and message on a line with the same text. They are not the edit's, so
    /// they are neither in `items` nor counted in `errors` and `warnings`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preexisting: Vec<DocDiagnostic>,
    /// "type annotations needed" on a `#[derive(...)]` line: the analyzer failing to type its
    /// own expansion of the derive (`serde::Deserialize` does it), which rustc does not report.
    /// Not counted, even in a new file that has no text on disk to compare with (#159).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub in_derive: Vec<DocDiagnostic>,
    /// E0277 that a type is not `Send`, `Sync` or `Unpin`, in a Rust file: rust-analyzer does
    /// not always prove an auto trait rustc proves (through a recursive `async fn`, #327). Shown,
    /// not counted; `cargo check` (`verify: "compile"`) decides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_trait: Vec<DocDiagnostic>,
    /// Intercepted hallucinations (roadmap 7.7): invalid method invocations, parameter/argument
    /// mismatches, borrow checker errors, and syntax anomalies detected on the fly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hallucinations: Vec<HallucinationInterception>,
}

impl DiagnosticsReport {
    pub fn is_platform_excluded(&self) -> Option<&str> {
        self.items
            .iter()
            .find(|d| d.code.as_deref() == Some("platform-excluded"))
            .map(|d| d.message.as_str())
    }

    pub fn ok(&self) -> bool {
        self.errors == 0
    }

    pub fn render(&self) -> String {
        if let Some(reason) = self.is_platform_excluded() {
            return format!("{}: platform-excluded ({})\n", self.file, reason);
        }
        let mut out = format!(
            "{}: {} error(s), {} warning(s)\n",
            self.file, self.errors, self.warnings
        );
        if !self.preexisting.is_empty() {
            out.push_str(&format!(
                "  ({} diagnostic(s) the file already had before this edit are not counted: {})\n",
                self.preexisting.len(),
                preexisting_summary(&self.preexisting)
            ));
        }
        if !self.in_derive.is_empty() {
            out.push_str(&format!(
                "  ({} \"type annotations needed\" on a #[derive(...)] line are not counted: the \
                 analyzer's own expansion of the derive, which rustc does not report)\n",
                self.in_derive.len()
            ));
        }
        if !self.auto_trait.is_empty() {
            out.push_str(&format!(
                "  ({} unproven bound(s) or analyzer limitation(s) are not counted: rust-analyzer does not \
                 always prove what rustc does; `cargo check` or `verify: \"compile\"` decides)\n",
                self.auto_trait.len()
            ));
            for d in &self.auto_trait {
                out.push_str(&format!(
                    "    unconfirmed: {} ({}:{}:{})\n",
                    d.message.lines().next().unwrap_or(""),
                    self.file,
                    d.line,
                    d.col
                ));
            }
        }
        for d in &self.items {
            out.push_str(&format!(
                "  {}: {}{} ({}:{}:{})\n",
                d.severity,
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                self.file,
                d.line,
                d.col
            ));
            if let Some(note) = &d.note {
                out.push_str(&format!("    note: {note}\n"));
            }
        }
        if !self.hallucinations.is_empty() {
            out.push_str("  === Intercepted Hallucinations (Phase 7.7) ===\n");
            for h in &self.hallucinations {
                out.push_str(&format!(
                    "  [INTERCEPT] {}: {} ({}:{}:{})\n",
                    h.kind,
                    h.message.lines().next().unwrap_or(""),
                    self.file,
                    h.line,
                    h.col
                ));
                if let Some(sugg) = &h.suggestion {
                    out.push_str(&format!("    --> {sugg}\n"));
                }
            }
        }
        out
    }
}

/// The distinct messages among `items`, most frequent first, each with how often it occurs.
pub fn preexisting_summary(items: &[DocDiagnostic]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for d in items {
        let message = format!(
            "{}{}",
            d.message.lines().next().unwrap_or(""),
            d.code
                .as_deref()
                .map(|c| format!(" [{c}]"))
                .unwrap_or_default()
        );
        match counts.iter_mut().find(|(m, _)| *m == message) {
            Some((_, n)) => *n += 1,
            None => counts.push((message, 1)),
        }
    }
    counts.sort_by_key(|a| std::cmp::Reverse(a.1));
    counts
        .iter()
        .map(|(m, n)| format!("{n}× {m}"))
        .collect::<Vec<_>>()
        .join(", ")
}
