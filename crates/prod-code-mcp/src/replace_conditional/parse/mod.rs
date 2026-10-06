/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod if_else;
pub mod rust_match;
pub mod switch;

pub use if_else::parse_if_else_block;
pub use rust_match::parse_rust_match;
pub use switch::parse_switch_block;
