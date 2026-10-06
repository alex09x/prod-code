/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

/// Who a session belongs to, for metrics.
pub struct SessionMeta {
    pub session_id: u64,
    pub client_name: String,
    pub agent: String,
    pub host: String,
    pub client_addr: String,
    pub workspace: String,
    pub engine: String,
    pub engine_root: PathBuf,
    pub storage_root: PathBuf,
    pub metrics: Arc<metrics::Metrics>,
    /// The session is an editor's ([`prod_code_protocol::PURPOSE_EDITOR`]): the Rust engine
    /// pushes the diagnostics of every document it opens or changes.
    pub editor: bool,
    /// Per document, how many edits the session has sent: a diagnostics pass waits out a burst
    /// of typing and runs only for the last edit of it.
    pub edits: Arc<std::sync::Mutex<std::collections::HashMap<PathBuf, u64>>>,
}

/// An LSP request awaiting its answer.
pub struct PendingRequest {
    pub method: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub start: Instant,
}
