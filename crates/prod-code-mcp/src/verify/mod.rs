/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod detect;
pub mod execute;
pub mod go_shadow;
pub mod parse;
pub mod plan;
mod render;
pub mod types;

#[cfg(test)]
mod tests;

pub use detect::*;
pub use execute::*;
pub use go_shadow::*;
pub use parse::*;
pub use plan::*;
pub use types::*;
