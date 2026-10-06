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

pub use polyglot::{
    find_cpp_instantiations, find_go_instantiations, find_python_instantiations,
    find_swift_instantiations, find_ts_instantiations,
};
pub use rust::find_rust_instantiations;
