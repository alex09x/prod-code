/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Managed Go language intelligence engine powered by supervised `gopls`.
//!
//! Provides multi-worktree Go code intelligence with shared GOCACHE and GOMODCACHE
//! for high-performance symbol resolution and compilation reuse.

pub mod config;
pub mod engine;
pub mod lsp;
mod probe;
mod reader;
pub(crate) mod types;

#[cfg(test)]
mod tests;

pub use config::{GoConfig, find_gopls_binary};
pub use engine::GoEngine;
