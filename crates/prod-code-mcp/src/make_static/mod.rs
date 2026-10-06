/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Turning a method that never uses `self` into an associated function, with every call site.
//!
//! The declaration loses its receiver; `value.method(args)` becomes `Type::method(args)` and
//! `Type::method(value, args)` loses its first argument. What cannot be done silently is dropping
//! a receiver that does something when it is evaluated — `load()?.method()` runs `load` — so such a
//! call site is reported, and nothing is written while one remains.

pub mod calls;
pub mod helpers;
pub mod polyglot;
pub mod rust;
pub mod types;

#[cfg(test)]
mod tests;

pub use helpers::{extract_receiver, find_method_at_line, receiver_has_effects, split_receiver};
pub use polyglot::make_static_polyglot;
pub use rust::make_static;
pub use types::{Language, MadeStatic};
