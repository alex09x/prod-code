/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Applying the compiler's own fixes from a failing `check` or `lint`, without a prompt.

pub mod apply;
pub mod parse;
pub mod plan;
pub mod types;

#[cfg(test)]
mod tests;

pub use apply::check_and_fix;
pub use parse::parse_fixes;
pub use plan::plan;
pub use types::{Edit, Fix, Fixed, Outcome};
