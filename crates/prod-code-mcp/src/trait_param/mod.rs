/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Removing a parameter of a trait method: from the trait's declaration, from every
//! implementation, and from every call.

pub mod execute;
pub mod syntax;
pub mod types;

#[cfg(test)]
mod tests;

pub use execute::remove_parameter;
pub use syntax::{effect_of, item_spans, mentions, owner_of, removal};
pub use types::{Owner, TraitParameter};
