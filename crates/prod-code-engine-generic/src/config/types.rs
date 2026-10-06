/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::readiness::{INDEX_WAIT, ReadySignal};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_MAX_RETAINED_DOCUMENTS: usize = 128;
pub const DEFAULT_HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(60);
pub const MAX_IDLE_PROBE_TIMEOUTS: usize = 3;
pub const HEALTH_PROBE_METHOD: &str = "prodCode/healthProbe";
pub const HEALTH_PROBE_ID_PREFIX: &str = "prod-code-health:";
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration for a generic LSP server adapter.
#[derive(Debug, Clone)]
pub struct GenericLspConfig {
    /// Command / binary name or path to execute (e.g. `pyright-langserver`, `ruff`, `vtsls`).
    pub command: String,
    /// Arguments to pass to the binary (e.g. `["--stdio"]`).
    pub args: Vec<String>,
    /// Environment variables to pass to the child process.
    pub env: HashMap<String, String>,
    /// Working directory for the server.
    pub working_dir: Option<PathBuf>,
    /// `initializationOptions` sent with the LSP `initialize` request.
    pub initialization_options: Option<serde_json::Value>,
    /// How long to wait for an answer before giving up on a request. Servers differ by more
    /// than an order of magnitude — a formatter answers instantly, a type checker on a cold
    /// project does not — so this is per server rather than one number for all of them.
    pub request_timeout: Duration,
    /// How the server tells that it has loaded and indexed its project, so that questions
    /// answered from its index wait for it instead of getting nothing or a part (#391).
    pub ready: ReadySignal,
    /// How long such a question waits for the server at most before it is asked anyway, with
    /// a note of how far the server got.
    pub index_wait: Duration,
    /// Keep documents open and restore their disk text with `didChange` when a client closes
    /// them. Pyright can retain distinct identities for `builtins.str` and `str` after a
    /// close/reopen cycle; changing the existing document avoids that corrupt state (#466).
    pub retain_open_documents: bool,
    /// Maximum logically closed documents retained in one server. At the limit the engine
    /// stops accepting new opens while existing owners may still change and close documents;
    /// its owner must replace the whole server rather than close/reopen one document.
    pub max_retained_documents: usize,
    /// How often an initialized, idle server is asked a private dispatch-only probe. `None`
    /// disables probes; the default is deliberately conservative for normal workspaces.
    pub health_probe_interval: Option<Duration>,
}

impl Default for GenericLspConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            working_dir: None,
            initialization_options: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            ready: ReadySignal::Unknown,
            index_wait: INDEX_WAIT,
            retain_open_documents: false,
            max_retained_documents: DEFAULT_MAX_RETAINED_DOCUMENTS,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }
}
