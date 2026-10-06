/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Language Server Protocol (LSP) editor adapter for rust-analyzer.
//!
//! Exposes editor endpoints (completion, hover, diagnostics, inlay hints, signature help,
//! document highlight, code actions, formatting) over typed JSON-RPC payloads.

pub mod engine;
pub mod format;
pub mod lines;
pub mod mapping;
pub mod snapshot;

#[cfg(test)]
mod tests;

pub use lines::Lines;
pub use snapshot::EDITOR_METHODS;
