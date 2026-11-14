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
    /// The client said it follows a `Redirect` frame; one that did not is disconnected with
    /// the new node's address when its workspace moves (#1495).
    pub redirects: bool,
    /// Per document, how many edits the session has sent: a diagnostics pass waits out a burst
    /// of typing and runs only for the last edit of it.
    pub edits: Arc<std::sync::Mutex<std::collections::HashMap<PathBuf, u64>>>,
    /// The editor requests still to be answered, for `$/cancelRequest`.
    pub cancels: Arc<RequestCancels>,
}

/// The requests of one session that its client may still cancel, by JSON-RPC id.
#[derive(Default)]
pub struct RequestCancels(
    std::sync::Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
);

impl RequestCancels {
    /// Registers the request `id` and returns the flag its query watches.
    pub fn begin(&self, id: &serde_json::Value) -> Arc<std::sync::atomic::AtomicBool> {
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), Arc::clone(&flag));
        flag
    }

    /// The request `id` was answered.
    pub fn finish(&self, id: &serde_json::Value) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id.to_string());
    }

    /// The client cancelled `id`. False when no such request is waiting: it was answered, or
    /// it is of a kind that is not cancelled.
    pub fn cancel(&self, id: &serde_json::Value) -> bool {
        let waiting = self.0.lock().unwrap_or_else(|e| e.into_inner());
        waiting.get(&id.to_string()).is_some_and(|flag| {
            flag.store(true, std::sync::atomic::Ordering::Release);
            true
        })
    }
}

/// An LSP request awaiting its answer.
pub struct PendingRequest {
    pub method: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub start: Instant,
}
