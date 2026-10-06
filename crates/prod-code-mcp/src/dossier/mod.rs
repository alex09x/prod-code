/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Failure dossier (roadmap 8.2): run the tests (or one filter), and for every failure
//! collect where it happened, the code there, the enclosing function's callers and what
//! changed in that file, so an agent gets the whole picture in one call.

pub mod assertions;
pub mod diagnose;
pub mod locations;
#[cfg(test)]
mod tests;
pub mod types;

pub use assertions::{parse_assertion_evidence, strip_ansi};
pub use diagnose::diagnose;
pub use locations::{locations_in, locations_in_with_hint, suggested};
pub use types::{AssertionEvidence, DossierReport, FailureDossier, FailureSite, Suspect};
