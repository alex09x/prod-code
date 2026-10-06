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

pub use self::helpers::{receiver_for, receiver_of, split_call_arguments};
pub use self::polyglot::{
    convert_to_method_polyglot, rewrite_static_calls_in_code, to_method_cpp, to_method_go,
    to_method_py, to_method_swift, to_method_ts,
};
pub use self::rust::convert_to_method;
pub use self::types::MadeMethod;
