/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Introducing a variable for every occurrence of an expression: `(w + 1)` three times in a
//! function becomes `let w1 = w + 1;` once, and `w1` three times.

pub mod analysis;
pub mod transform;
pub mod types;

#[cfg(test)]
mod tests;

pub use analysis::{
    can_panic, changes, enclosing_body, innermost_block, loops_after, occurrences, parenthesised,
    reads_and_effects, statement_start, surely_evaluated,
};
pub use transform::introduce_variable;
pub use types::Introduced;
