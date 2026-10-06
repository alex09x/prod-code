/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Turning a loop that only builds up an accumulator into an iterator chain: `let mut sum = 0;
//! for p in prices { sum += p * 2; }` becomes `let sum: u64 = prices.into_iter().map(|p| p * 2)
//! .sum();`.
//!
//! rust-analyzer offers `for_each` and `while let`, which keep the mutable accumulator. This
//! recognises the shapes whose chain means the same: a sum from zero, a count of matches, and a
//! vector built with `push`, each with or without an `if` around the one statement. `for P in E`
//! iterates `E.into_iter()`, and the closure binds `P` exactly as the loop did. Anything else in
//! the body (a second statement, `break`, `?`, another use of the accumulator) is refused, and
//! the result is type-checked before anything is written.

pub mod helpers;
pub mod polyglot;
pub mod rust;
pub mod types;

#[cfg(test)]
mod tests;

pub use helpers::{binding_type, find_loop_offset};
pub use polyglot::{
    loop_to_iterator_polyglot, recognise_cpp, recognise_go, recognise_python, recognise_swift,
    recognise_ts,
};
pub use rust::{chain, iterator_of, loop_to_iterator, recognise};
pub use types::{AccumulatorLoop, PolyglotLoop, Rewritten, Shape};
