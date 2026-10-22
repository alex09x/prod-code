/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod baseline;
pub mod batch;
mod budget;
mod engine;
pub mod single;

#[cfg(test)]
mod baseline_tests;
#[cfg(test)]
mod tests;

pub use batch::*;
pub use single::*;
