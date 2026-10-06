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
pub mod generics;
pub mod items;
pub mod lex;
pub mod rewrite;
pub mod types;

#[cfg(test)]
mod tests;

pub use execute::{extract_trait, extract_trait_ext};
pub use generics::impl_body_open;
pub use items::{impl_block, items};
pub use rewrite::{declaration, rewrite, signature, visibility};
pub use types::{Extracted, ImplBlock, Item, is_ident, valid_ident};
