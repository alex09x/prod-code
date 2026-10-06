/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Multi-language workspace detection.
//!
//! Automatically identifies the language engine suited for a given workspace
//! based on project manifests (Cargo.toml, go.mod, package.json, pyproject.toml, etc.).

pub mod all;
pub mod heuristics;
pub mod markers;
pub mod primary;

pub use all::detect_all_engines;
pub use heuristics::*;
pub use markers::*;
pub use primary::{detect_engine, resolve_engine};

#[cfg(test)]
mod tests;
