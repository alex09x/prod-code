/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod execute;
pub mod parse;
pub mod transform;
pub mod types;

#[cfg(test)]
mod tests;

pub use execute::replace_conditional_impl;
pub use parse::{parse_if_else_block, parse_rust_match, parse_switch_block};
pub use transform::{
    transform_cpp, transform_python, transform_rust, transform_swift, transform_typescript,
};
pub use types::{
    ConditionalBlock, ConditionalBranch, ConditionalKind, ReplaceConditionalResult,
    line_col_to_offset, line_indentation, returns_from_conditional, tag_to_variant_name,
};
