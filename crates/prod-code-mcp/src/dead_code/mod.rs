/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Dead-code scan (roadmap 8.6): functions, methods and types nobody references, found by
//! asking the analyzer for the references of every symbol in the checkout.
//!
//! Only a successful answer with an empty list of references makes a symbol dead. A request
//! that failed, a `null` (the protocol's "no result", which does not say nothing references it)
//! or an answer of another shape leaves the symbol unverified, never dead, and so out of reach of
//! pruning (#435). The same holds for a file's symbols: a list with an entry that cannot be read
//! leaves the whole file unverified, since a symbol skipped is a symbol never judged.

pub mod reachability_scan;
pub mod scan;
pub mod syntax;
pub mod types;

#[cfg(test)]
mod tests;

pub use scan::{find_dead_code, find_dead_code_opts};
pub use syntax::{collect_candidates, is_exported, reference_count};
pub use types::{CandidateSymbol, DeadCodeOptions, DeadCodeReport, DeadItem, Unverified};
