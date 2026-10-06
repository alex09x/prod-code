/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Blast radius of a change (roadmap 8.1): the functions a diff touches, the callers that
//! reach them through the analyzer's call hierarchy, and the tests among those callers,
//! with the command that runs only the affected tests.
//!
//! What the analysis could not establish (a deleted file, a change whose lines cannot be placed,
//! a request that failed, an answer it cannot read, a walk cut short by the depth limit) is
//! reported as a gap, never read as "no callers": a selection with a gap is not trusted, and
//! `impact --ci` runs the whole suite (#434).

pub mod analyze;
pub mod call_sites;
pub mod diff;
pub mod incoming;
pub mod pool;
pub mod render;
pub mod rust_attr;
pub mod signatures;
pub mod symbols;
pub mod test_cmd;
pub mod types;

#[cfg(test)]
mod tests;

pub use analyze::analyze;
pub use diff::changed_lines;
pub use incoming::{suspects_for, unreadable};
pub use symbols::{is_scratch_path, one_based};
pub use test_cmd::{
    file_language, looks_like_test, registration, test_command, test_command_for_tests, test_marker,
};
pub use types::*;
