/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Fixture generation: compile-ready values for types, built from shapes
//! reported by analyzers and checked by analyzers.

pub mod builder;
pub mod generate;
pub mod mock;
pub mod polyglot;
pub mod resolve;
pub mod types;
pub mod values;

#[cfg(test)]
mod tests;

pub use generate::*;
pub use resolve::*;
pub use types::*;
pub use values::*;
