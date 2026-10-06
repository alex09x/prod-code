/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// One proposed file of a hypothesis; `text: None` deletes the file.
#[derive(Debug, Clone)]
pub struct HypothesisEdit {
    /// `/`-separated path relative to the workspace root.
    pub relative_path: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HypothesisSpec {
    pub name: String,
    pub edits: Vec<HypothesisEdit>,
}

/// What happened to one hypothesis, with the diff it stands for.
#[derive(Debug, Clone)]
pub struct HypothesisOutcome {
    pub name: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub error: Option<String>,
    /// Tail of the combined output (lossy UTF-8).
    pub output: String,
    pub output_len: u64,
    /// (passed, failed) when the command's output could be parsed as a test run.
    pub tests: Option<(u64, u64)>,
    pub diff: String,
    pub changed_lines: usize,
}

impl HypothesisOutcome {
    pub fn passed(&self) -> bool {
        self.error.is_none() && !self.timed_out && self.exit_code == Some(0)
    }
}

#[derive(Debug, Clone)]
pub struct ShadowOutcome {
    pub mode: String,
    pub server_workspace_root: String,
    pub results: Vec<HypothesisOutcome>,
    /// Indices into `results`, best first.
    pub ranking: Vec<usize>,
    /// The best hypothesis when it passed.
    pub winner: Option<usize>,
    /// Whether the shadow run executed in an in-memory RAM overlay.
    pub in_memory: bool,
}
