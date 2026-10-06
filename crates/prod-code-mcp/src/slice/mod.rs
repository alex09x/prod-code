/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod candidates;
pub(crate) mod execute;
pub(crate) mod facts;
pub(crate) mod lsp;
pub mod types;

#[cfg(test)]
mod tests;

pub use self::candidates::candidate_names;
pub use self::execute::{slice, slice_with_options};
pub use self::types::{
    DEFAULT_DEPTH, DEFAULT_MAX_BYTES, GapKind, SliceGap, SliceItem, SliceOptions, SliceReport,
};
