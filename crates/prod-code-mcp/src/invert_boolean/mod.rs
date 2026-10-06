/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Inverting a predicate: a function returning `bool` gets the opposite name and meaning, and every
//! caller keeps doing what it did.
//!
//! `is_valid` becomes `is_invalid`: the body returns the negation of what it returned, and every call
//! becomes `!is_invalid(…)` — or loses the `!` it already had, since two negations cancel. Nothing is
//! renamed or negated textually by name: the calls are the analyzer's references, and the result is
//! type-checked before anything is written.

pub mod decl;
pub mod negate;
pub mod polyglot;
pub mod rust;
pub mod syntax;
pub mod types;

#[cfg(test)]
mod tests;

pub use polyglot::invert_polyglot;
pub use rust::invert;
pub use syntax::own_returns;
pub use types::Inverted;
