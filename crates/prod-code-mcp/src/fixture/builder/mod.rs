/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Typed builders (roadmap 8.5, #459): the source of a builder for a struct with named fields,
//! read from the declaration in the file and checked by the analyzer where it would go.

pub mod codegen;
pub mod generics;
pub mod lexer;
pub mod outline;
pub mod parser;
pub mod plan;
pub mod preview;
pub mod types;
pub mod verify;

#[cfg(test)]
mod tests_codegen;
#[cfg(test)]
mod tests_plan;

pub use plan::plan;
pub use preview::preview;
pub use types::{BuilderField, BuilderPlan, BuilderPreview, BuilderRequest, Verification};
