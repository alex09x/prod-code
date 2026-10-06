/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Intra-function backward data-flow and control-dependency program slicing (Roadmap 7.3).
//!
//! Given a function body and a slicing criterion `(target_line, target_var)`, this module
//! computes the minimal set of statements that affect the target value by traversing
//! data dependencies (def-use chains) and control dependencies (conditional branches and loops)
//! backwards within the function.
//!
//! Produces an explicit completeness contract (`Complete`, `Bounded`, or `Incomplete`)
//! so callers can distinguish mathematically verified dependency closures from partial cuts.

pub(crate) mod keywords;
pub(crate) mod line_analyzer;
pub(crate) mod parser;
pub mod slice;
pub mod types;

#[cfg(test)]
mod tests;

pub use slice::slice_intra_function;
pub use types::{DataFlowSlice, SliceCompleteness, SliceStatement};
