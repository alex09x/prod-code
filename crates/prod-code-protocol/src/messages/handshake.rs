/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};

/// Client capabilities advertised during handshake.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub direct_edit: bool,
    #[serde(default)]
    pub watch_files: bool,
    #[serde(default)]
    pub indexing_status: bool,
    #[serde(default)]
    pub shadow_runs: bool,
    #[serde(default)]
    pub multi_root: bool,
    #[serde(default)]
    pub sync_chunking: bool,
    #[serde(default)]
    pub unix_socket_local: bool,
    /// The client can decode transparent gateway redirect frames during handshakes.
    #[serde(default)]
    pub redirects: bool,
}

/// Server capabilities granted during handshake.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerCapabilities {
    #[serde(default)]
    pub direct_edit: bool,
    #[serde(default)]
    pub watch_files: bool,
    #[serde(default)]
    pub indexing_status: bool,
    #[serde(default)]
    pub shadow_runs: bool,
    #[serde(default)]
    pub multi_root: bool,
    #[serde(default)]
    pub sync_chunking: bool,
    #[serde(default)]
    pub unix_socket_local: bool,
}

/// Initial handshake request sent by client upon connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeRequest {
    pub protocol_version: u32,
    /// Versions this client can actually speak. Absent means the legacy singleton offer in
    /// `protocol_version`; present-but-empty is invalid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_versions: Option<Vec<u32>>,
    /// Negotiated client capabilities offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ClientCapabilities>,
    pub client_name: String,
    pub client_pid: u32,
    pub auth_token: Option<String>,
    pub client_workspace_root: String,
    #[serde(default)]
    pub preferred_engine: Option<String>,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
    /// Directory inside the checkout (relative, `/`-separated) whose project the session is
    /// about, when it is a nested project of another language than the checkout root
    /// (a SwiftPM package inside a Rust repository): the gateway loads the engine there.
    #[serde(default)]
    pub engine_subpath: Option<String>,
    /// What drives this client: `claude-code`, `codex`, `cli`, or a custom `PROD_CODE_AGENT`.
    #[serde(default)]
    pub client_agent: Option<String>,
    /// The client machine's hostname.
    #[serde(default)]
    pub client_host: Option<String>,
    /// What the session is for, when that changes where it should run. [`PURPOSE_VALIDATION`]:
    /// the session opens proposed texts only to ask what the analyzer thinks of them, so the
    /// gateway serves it from a second engine for the same workspace, and the overlay and its
    /// revert never invalidate what the main engine has computed (#73).
    #[serde(default)]
    pub purpose: Option<String>,
    /// Number of times this handshake has been transparently redirected across nodes (Roadmap 5.1).
    #[serde(default)]
    pub redirect_count: u32,
}

/// Code of the diagnostic a gateway reports for a file the analyzer panicked on (#94): the file
/// was not checked at all. It is never set aside as a diagnostic the file already had.
pub const ANALYZER_PANIC_CODE: &str = "prod-code::analyzer-panic";

/// [`HandshakeRequest::purpose`] of a session that only validates proposed texts.
pub const PURPOSE_VALIDATION: &str = "validation";

/// [`HandshakeRequest::purpose`] of an editor's session (`prod-code lsp`): the gateway pushes
/// the diagnostics of the documents it opens and changes, as a language server does (#310).
pub const PURPOSE_EDITOR: &str = "editor";

/// Handshake acknowledgement sent by remote gateway.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeResponse {
    pub protocol_version: u32,
    pub server_pid: u32,
    pub session_id: u64,
    pub server_workspace_root: String,
    pub detected_engine: String,
    /// Files the gateway removed from its copy of the workspace because a command changed them
    /// after its client left and their old contents were not kept (#262). The client drops them
    /// from its sync watermark for this node, so that its next sync sends them again.
    #[serde(default)]
    pub stale_paths: Vec<String>,
    /// How long ago, in milliseconds, the gateway loaded the engine this session attaches to.
    /// `None` from a gateway too old to say, and for an editor's own server. An empty
    /// `workspace/symbol` from an engine loaded moments ago may be early and is asked again; one
    /// from a warm engine is the answer (#381).
    #[serde(default)]
    pub engine_age_ms: Option<u64>,
    /// Whether the gateway holds this engine's index questions (`workspace/symbol`,
    /// `references`, ...) until its server has loaded and indexed, and otherwise sends a
    /// `prod-code/indexing` note with the answer: then an empty answer is final. `false` from a
    /// gateway too old to do so, and for a server whose readiness is not known (#391).
    #[serde(default)]
    pub index_gated: bool,
    /// Negotiated server capabilities granted to this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ServerCapabilities>,
}
