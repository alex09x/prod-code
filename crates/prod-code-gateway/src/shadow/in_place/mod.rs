/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod lock;
pub mod runner;

pub use runner::run_in_place;

#[cfg(test)]
pub(crate) use runner::{FillFault, inject_fill_fault};
