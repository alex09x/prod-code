/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Pluggable Generic LSP engine for external language servers (Pyright, Ruff, TypeScript, etc.).
//!
//! Provides supervised process lifecycle, automatic framing, request/response routing,
//! health monitoring, and idle shutdown management.

pub mod config;
pub mod diagnostics;
pub mod documents;
pub mod engine;
pub mod lsp;
pub mod probe;
pub mod reader;
pub mod types;

#[cfg(test)]
mod tests;

pub use config::{
    DEFAULT_HEALTH_PROBE_INTERVAL, DEFAULT_MAX_RETAINED_DOCUMENTS, DEFAULT_REQUEST_TIMEOUT,
    GenericLspConfig, settings_for_section, venv_python, which_bin,
};
pub use diagnostics::{DiagnosticsUnavailable, Unavailable};
pub use engine::GenericLspEngine;
