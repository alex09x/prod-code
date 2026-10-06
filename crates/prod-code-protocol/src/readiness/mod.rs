/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Whether a language server has finished loading and indexing, from what it says itself: the
//! work-done progress it begins and ends (`$/progress`), or, for a server that reports none, the
//! log line that says it is set up. Asked before that, clangd answered `workspace/symbol` with
//! nothing and then with the part of the index built so far, and basedpyright with nothing (#391).

mod tracker;
mod types;

#[cfg(test)]
mod tests;

pub use tracker::Readiness;
pub use types::{
    BUSY_MEMBER, BUSY_NOTIFICATION, Busy, INDEX_WAIT, ReadySignal, needs_index,
    pyright_found_sources,
};
