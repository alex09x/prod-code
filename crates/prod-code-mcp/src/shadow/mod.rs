/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Shadow runs from the client side (roadmap 7.4): send named hypotheses (complete proposed
//! file contents) to the gateway, which runs a command once per hypothesis in a private shadow
//! of the workspace; rank the outcomes and describe the winner as a unified diff against the
//! checkout. Shared by the MCP tool `code_shadow_run` and the CLI `prod-code shadow-run`.

pub mod diff;
pub(crate) mod execute;
pub mod rank;
pub mod report;
mod retry;
pub mod spec;
pub mod types;

#[cfg(test)]
mod tests;

pub use diff::{apply_hypothesis, unified_diff};
pub(crate) use execute::run_shadow_once;
pub use rank::{rank, test_counts};
pub use report::render_report;
pub use retry::run_shadow;
pub use spec::{parse_specs, relative_edit_path};
pub use types::{HypothesisEdit, HypothesisOutcome, HypothesisSpec, ShadowOutcome};
