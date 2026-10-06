/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod cgo;
pub mod detect;
pub mod project;
pub mod workspace;

pub use cgo::macos_only_cgo;
pub use project::{engine_for_file, engine_project, expected_engine, other_checkout};
pub use workspace::is_in_dependency_dir;
