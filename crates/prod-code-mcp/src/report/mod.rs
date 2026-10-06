/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Reporting a bug in prod-code itself as a GitHub issue, from the agent that hit it (#290).

pub mod gh;
pub mod scrub;
pub mod types;

#[cfg(test)]
mod tests;

pub use gh::{gh_program, report};
pub use scrub::{draft, environment, scrub};
pub use types::{
    AREA_LABELS, DUPLICATES_SHOWN, Draft, Outcome, REPOSITORY, ReportRequest, Similar, TYPE_LABELS,
    issue_labels,
};
