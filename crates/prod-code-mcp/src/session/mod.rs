/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A persistent gateway session for batches of LSP queries: one connection, one pre-flight
//! sync, one handshake and one `initialize`, then any number of requests (documents are
//! opened once). Batch features (impact analysis, dead-code scans) use this instead of a
//! connection per query.

mod connection;
mod methods;
mod pool;
#[cfg(test)]
mod tests;
mod types;

pub use pool::{pooled_engine_age, pooled_index_gated, pooled_query};
pub use types::{INDEXING_GRACE, LspSession, take_indexing_notes};
