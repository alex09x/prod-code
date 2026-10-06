/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Multi-language fixture parsing and value generation (Roadmap 8.5).

pub mod format;
pub mod parsers_go_ts;
pub mod parsers_others;
pub mod samples;
pub mod types;

#[cfg(test)]
mod tests;

pub use format::*;
pub use parsers_go_ts::*;
pub use parsers_others::*;
pub use samples::*;
pub use types::*;
