/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Inverting a boolean field or local variable: `enabled` becomes `disabled`, and every place that
//! reads it or writes it keeps doing what it did.

pub mod lsp;
pub mod syntax;
pub mod transform;
pub mod types;

#[cfg(test)]
mod tests;

pub use lsp::hover_type;
pub use transform::invert_value;
pub use types::{ValueKind, value_kind};
