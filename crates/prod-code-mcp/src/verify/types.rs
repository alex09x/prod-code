/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub use super::tooling::{
    CppBuild, JsTestRunner, PackageManager, ProjectTools, PythonRuntime, PythonTestRunner,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyKind {
    Check,
    Lint,
    Test,
    Bench,
}

impl VerifyKind {
    pub fn label(&self) -> &'static str {
        match self {
            VerifyKind::Check => "check",
            VerifyKind::Lint => "lint",
            VerifyKind::Test => "test",
            VerifyKind::Bench => "bench",
        }
    }
}

/// One benchmark's result: its estimate and, when the harness gives one, the range around it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BenchResult {
    pub name: String,
    /// `2.3500 ns`, `1234 ns/iter`, `1234 ns/op`: as the harness prints it.
    pub estimate: String,
    /// `2.3499 ns .. 2.3502 ns` (criterion's interval) or `+/- 56` (libtest).
    pub range: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub level: String,
    pub code: Option<String>,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u64>,
    pub column: Option<u64>,
}

impl Diagnostic {
    pub fn render(&self) -> String {
        let at = match (&self.file, self.line, self.column) {
            (Some(f), Some(l), Some(c)) => format!("{f}:{l}:{c}"),
            (Some(f), Some(l), None) => format!("{f}:{l}"),
            (Some(f), _, _) => f.clone(),
            _ => String::new(),
        };
        let code = self
            .code
            .as_ref()
            .map(|c| format!("[{c}] "))
            .unwrap_or_default();
        if at.is_empty() {
            format!("{}: {code}{}", self.level, self.message)
        } else {
            format!("{}: {code}{} ({at})", self.level, self.message)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestFailure {
    pub name: String,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    pub kind: VerifyKind,
    pub language: String,
    pub command: Vec<String>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub diagnostics: Vec<Diagnostic>,
    pub tests_passed: u64,
    pub tests_failed: u64,
    pub failures: Vec<TestFailure>,
    /// Tail of the raw combined output, for anything the parsers did not understand.
    pub tail: String,
    /// The compiler's machine-applicable fixes (Rust check and lint), for `fix: true`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<crate::fixit::Fix>,
    /// Benchmark results, for a `bench` run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub benches: Vec<BenchResult>,
    /// CPU time and peak memory of the command and its children, when the node could tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<prod_code_protocol::ExecUsage>,
    /// The OS and architecture the run was on (`linux x86_64`): a diagnostic or a fix is for
    /// that platform (#140).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }

    pub fn is_toolchain_msrv_rejection(&self) -> bool {
        if self.ok() {
            return false;
        }
        let matches_msrv = |s: &str| {
            s.contains("is not supported by the following packages")
                || s.contains("requires at least rustc")
                || s.contains("requires rustc")
                || s.contains("package requires rustc")
        };
        self.diagnostics.iter().any(|d| matches_msrv(&d.message)) || matches_msrv(&self.tail)
    }

    pub fn errors(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.level == "error")
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.level == "warning")
            .count()
    }

    pub fn summary(&self) -> String {
        let status = match (self.timed_out, self.exit_code) {
            (true, _) => "TIMED OUT".to_string(),
            (false, Some(0)) => "OK".to_string(),
            (false, Some(code)) => format!("FAILED (exit {code})"),
            (false, None) => "KILLED".to_string(),
        };
        let mut parts = vec![format!(
            "{} {}: {status} in {:.1}s{}",
            self.language,
            self.kind.label(),
            self.duration_ms as f64 / 1000.0,
            self.platform
                .as_deref()
                .map(|p| format!(" on {p}"))
                .unwrap_or_default()
        )];
        if self.kind == VerifyKind::Test {
            parts.push(format!(
                "{} passed, {} failed",
                self.tests_passed, self.tests_failed
            ));
        }
        if self.kind == VerifyKind::Bench {
            parts.push(format!("{} benchmark(s)", self.benches.len()));
        }
        if !self.diagnostics.is_empty() {
            parts.push(format!(
                "{} error(s), {} warning(s)",
                self.errors(),
                self.warnings()
            ));
        }
        if let Some(usage) = &self.usage {
            parts.push(usage.render());
        }
        parts.join("; ")
    }

    /// Human/agent readable report: summary, diagnostics, test failures, then the raw tail
    /// only when nothing structured explains a failure.
    pub fn render(&self, max_items: usize) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "$ {}\n{}\n",
            self.command.join(" "),
            self.summary()
        ));
        let diag_budget = if self.failures.is_empty() {
            MAX_RENDER_BYTES
        } else {
            // Reserve space for structured test failures so verbose compiler warnings
            // or diagnostics never crowd out failed-test details and traces.
            MAX_RENDER_BYTES / 2
        };
        let rendered_diags =
            super::render::append_diagnostics(&mut out, &self.diagnostics, max_items, diag_budget);
        for b in self.benches.iter().take(max_items) {
            if out.len() >= MAX_RENDER_BYTES {
                break;
            }
            out.push_str(&format!("  {}  {}", b.name, b.estimate));
            if let Some(range) = &b.range {
                out.push_str(&format!("  [{range}]"));
            }
            out.push('\n');
        }
        let mut rendered_failures = 0;
        for f in self.failures.iter().take(max_items) {
            if out.len() >= MAX_RENDER_BYTES {
                break;
            }
            let formatted_output = format_failure_output(&f.output, MAX_FAILURE_OUTPUT_BYTES);
            out.push_str(&format!(
                "--- FAILED {} ---\n{}\n",
                f.name, formatted_output
            ));
            rendered_failures += 1;
        }
        if self.failures.len() > rendered_failures {
            out.push_str(&format!(
                "... {} more failed test(s)\n",
                self.failures.len() - rendered_failures
            ));
        }
        if !self.ok() && self.diagnostics.is_empty() && self.failures.is_empty() {
            out.push_str("--- output tail ---\n");
            let formatted_tail = format_failure_output(&self.tail, MAX_FAILURE_OUTPUT_BYTES);
            out.push_str(&formatted_tail);
            out.push('\n');
        }
        if out.len() > MAX_RENDER_BYTES {
            let truncated = truncate_to_boundary(&out, MAX_RENDER_BYTES);
            let mut capped = truncated.to_string();
            let omitted_failures = self.failures.len().saturating_sub(rendered_failures);
            if omitted_failures > 0 && !capped.contains("more failed test(s)") {
                if !capped.ends_with('\n') {
                    capped.push('\n');
                }
                capped.push_str(&format!("... {} more failed test(s)\n", omitted_failures));
            }
            let omitted_diags = self.diagnostics.len().saturating_sub(rendered_diags);
            if omitted_diags > 0 && !capped.contains("more diagnostic(s)") {
                if !capped.ends_with('\n') {
                    capped.push('\n');
                }
                capped.push_str(&format!("  ... {} more diagnostic(s)\n", omitted_diags));
            }
            capped.push_str("\n[... output truncated to avoid exceeding MCP line limits]\n");
            return capped;
        }
        out
    }
}

pub const MAX_FAILURE_OUTPUT_BYTES: usize = 4 * 1024;
pub const MAX_RENDER_BYTES: usize = 24 * 1024;

pub fn truncate_to_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        s
    } else {
        let mut idx = max_bytes;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        &s[..idx]
    }
}

pub fn tail_from_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        s
    } else {
        let mut idx = s.len() - max_bytes;
        while idx < s.len() && !s.is_char_boundary(idx) {
            idx += 1;
        }
        &s[idx..]
    }
}

pub fn format_failure_output(raw: &str, max_bytes: usize) -> String {
    let trimmed = raw.trim_end();
    if trimmed.len() <= max_bytes {
        return trimmed.to_string();
    }
    let head_limit = max_bytes / 4;
    let tail_limit = max_bytes.saturating_sub(head_limit);
    let head = truncate_to_boundary(trimmed, head_limit);
    let tail = tail_from_boundary(trimmed, tail_limit);
    let omitted = trimmed.len().saturating_sub(head.len() + tail.len());
    format!("{head}\n[... {omitted} bytes truncated ...]\n{tail}")
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunEvent {
    Diagnostic(Diagnostic),
    Test { name: String, ok: bool },
}
