/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Multi-worktree divergence and correctness benchmark.

mod report;
mod runner;
mod session;
mod setup;
mod target;
mod types;
mod verify;

#[cfg(test)]
mod tests;

pub use report::DivergentBenchReport;
pub use runner::run;
pub use session::initial_sync;
pub use setup::{bench_workspace_name, detect_language, setup};
pub use target::{discover_target, mutate_signature};
pub use types::{
    BENCH_WORKSPACE_SUFFIX, DivergenceSetup, DivergentBenchConfig, DivergentTarget,
    DivergentWorktree, Expectations, Language, MIN_WORKERS, QueryOutcome, SyncSummary,
    VerificationResult, WorkspaceMode, WorktreeKind,
};
pub use verify::verify;
