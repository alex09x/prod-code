/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod helpers;
pub mod polyglot;
pub mod rust;
pub mod types;

#[cfg(test)]
mod tests;

pub(crate) use helpers::is_in_literal_or_comment;
#[cfg(test)]
pub(crate) use helpers::mentions;
pub use helpers::self_type;
pub use polyglot::extract_polyglot;
pub use rust::{
    braces_kind, constructor_brace, extract, field_insertion, impl_blocks, literal_insertion,
    method_at, self_literals, struct_braces,
};
pub use types::{Braces, ExtractedField};
