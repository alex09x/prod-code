/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod binding;
pub mod duplicates;
pub mod execute;
pub mod rewrite;
#[cfg(test)]
mod tests;
pub mod tokens;
pub mod types;

pub use binding::{bound_names, read_after_but_not_returned};
pub use duplicates::copies_of;
pub use execute::extract_function;
pub use rewrite::{literal_type, rewrite_of, with_arguments};
pub use tokens::{Token, is_ident, mentions, tokens};
pub use types::{Duplicate, Extracted, Occurrence, PLACEHOLDER, Rewrite};
