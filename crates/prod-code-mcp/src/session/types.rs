/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::anyhow;
use prod_code_protocol::{AnyStream, ProdCodeCodec};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio_util::codec::Framed;

pub struct LspSession {
    pub remote: SocketAddr,
    pub(crate) framed: Framed<AnyStream, ProdCodeCodec>,
    pub(crate) root: PathBuf,
    /// The documents open in the server, by URI: the file, and a hash of the disk text last
    /// sent for it (`None` for a proposed text, which is not the file's).
    pub(crate) opened: HashMap<String, (PathBuf, Option<u64>)>,
    pub(crate) next_id: i64,
    /// The engine the gateway chose for this session.
    pub engine: String,
    /// When the gateway loaded that engine; `None` when it does not say (#381).
    pub(crate) engine_loaded: Option<std::time::Instant>,
    /// Whether the gateway holds this engine's index questions until its server is ready, so
    /// that an empty answer is final (#391).
    pub(crate) index_gated: bool,
}

/// How long after its engine was loaded an empty `workspace/symbol` answer may still be early:
/// a language server indexes after it starts (#381).
pub const INDEXING_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

// Includes connection, sync and cold engine loading. A dead gateway must not hold a caller
// indefinitely before its first LSP request even starts (#430).
pub(crate) const OPEN_BUDGET: std::time::Duration = std::time::Duration::from_secs(180);

pub(crate) fn timeout_error(stage: &str, budget: std::time::Duration) -> anyhow::Error {
    anyhow!(
        "timeout {stage} after {} ms; the gateway may be loading or short of capacity. \
             Run `prod-code cluster` to inspect it, then retry or select another node",
        budget.as_millis()
    )
}

/// A hash of a document's text, to tell whether the file still holds what was sent.
pub(crate) fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// How long a request may take before the session gives up on it. A structural rewrite searches
/// the workspace with type inference and legitimately takes minutes. A file's diagnostics are a
/// full check of it: 85 s cold for a 4,246-line file on an aarch64 build node, where 60 s made
/// the client give up and send the same work again (#237). Everything else is an interactive
/// query and should not take long.
pub(crate) fn budget_for(method: &str) -> std::time::Duration {
    std::time::Duration::from_secs(match method {
        "prodCode/structuralReplace" => 900,
        "textDocument/diagnostic" => 300,
        // The gateway may hold an index question while the server finishes indexing (#391).
        m if prod_code_protocol::readiness::needs_index(m) => 120,
        _ => 60,
    })
}

/// Notes, per checkout, of index questions the gateway answered while the language server was
/// still loading or indexing; a tool's answer carries them (#391).
static INDEXING_NOTES: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, Vec<String>>>> =
    std::sync::LazyLock::new(Default::default);

pub(crate) fn record_indexing_note(root: &Path, note: String) {
    let mut notes = INDEXING_NOTES.lock().unwrap_or_else(|e| e.into_inner());
    let notes = notes.entry(root.to_path_buf()).or_default();
    if !notes.contains(&note) {
        notes.push(note);
    }
}

/// The indexing notes recorded for the checkout at `root` since the last call.
pub fn take_indexing_notes(root: &Path) -> Vec<String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    INDEXING_NOTES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&root)
        .unwrap_or_default()
}
