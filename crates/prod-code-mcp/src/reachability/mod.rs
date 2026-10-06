/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Whole-program graph reachability analysis from entry points (roadmap 7.5, 8.6).

pub mod classify;
pub mod cycle;
pub mod graph;
pub mod types;

#[cfg(test)]
mod tests;

pub use classify::is_root_entry_point;
pub use cycle::detect_cycle;
pub use graph::ReachabilityGraph;
pub use types::{
    ReachabilityResult, ReachabilitySummary, SymbolDecl, SymbolKey, UnreachableCluster,
};
