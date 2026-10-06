/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Moving a method to the type of one of its parameters: `Order::price_with(&self, tax: &Tax)`
//! becomes `Tax::price_with(&self, order: &Order)`.
//!
//! The parameter becomes the receiver, borrowed as it was (`&Tax` → `&self`), and the old
//! receiver becomes a parameter in its place, typed as it was borrowed (`&self` → `&Order`). In
//! the body `self` becomes that parameter, the parameter becomes `self`, and `Self`, which meant
//! the old type, is spelled out. The method goes into the new type's inherent `impl` (one is
//! made after the type when there is none), and every call swaps the two:
//! `o.price_with(t, 1)` → `t.price_with(&o, 1)`, `Order::price_with(o, t, 1)` →
//! `Tax::price_with(t, o, 1)`. `&o` is right even when `o` is already a reference: an argument
//! of type `&&Order` coerces to `&Order`. The whole change is type-checked in one overlay first.

mod associated;
mod edits;
mod method;
mod rewrite;
mod syntax;
#[cfg(test)]
mod tests;
mod types;

pub use associated::move_associated_function;
pub use edits::cut_from_impl;
pub use method::move_method;
pub use syntax::{receiver_as_type, snake_case, swap_names};
pub use types::MovedMethod;
