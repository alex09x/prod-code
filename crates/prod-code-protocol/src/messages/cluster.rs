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

use super::status::StatusResponse;
use super::sync::base64_bytes;

/// The token a cluster's connections open with (#402). Its `Debug` never shows it, so a
/// message that is logged cannot leak it.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct AuthToken(pub String);

impl std::fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthToken(<redacted>)")
    }
}

impl AuthToken {
    /// Whether `expected` is this token, compared in a time that does not depend on where the
    /// two differ.
    pub fn matches(&self, expected: &str) -> bool {
        let (given, expected) = (self.0.as_bytes(), expected.as_bytes());
        given.len() == expected.len()
            && given
                .iter()
                .zip(expected)
                .fold(0u8, |differ, (a, b)| differ | (a ^ b))
                == 0
    }
}

/// A request to read a file on the gateway host, for definitions that resolve outside the
/// checkout (toolchain sources, dependency caches, system headers).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadFileRequest {
    /// Absolute path on the gateway host.
    pub path: String,
    /// Upper bound on the bytes returned; 0 means the server default.
    #[serde(default)]
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadFileResponse {
    pub path: String,
    /// The file's bytes (base64 on the wire), or None when it could not be read.
    #[serde(default, with = "base64_bytes")]
    pub content: Option<Vec<u8>>,
    #[serde(default)]
    pub truncated: bool,
    /// Whether the source file has any executable permission bit set on the gateway.
    /// `None` indicates a legacy gateway that did not report file permissions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_executable: Option<bool>,
    #[serde(default)]
    pub error: Option<String>,
}

/// A workspace a gateway currently holds in memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoadedWorkspaceInfo {
    pub name: String,
    pub engine: String,
    pub sessions: usize,
}

/// One gateway's heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeGossip {
    /// The address other nodes and clients reach this gateway at (`host:port`).
    pub addr: String,
    pub status: StatusResponse,
    #[serde(default)]
    pub workspaces: Vec<LoadedWorkspaceInfo>,
    /// Every peer address this node knows, so membership spreads transitively.
    #[serde(default)]
    pub peers: Vec<String>,
    #[serde(default)]
    pub sent_at_ms: u64,
}

/// A peer as seen by the answering node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerInfo {
    pub addr: String,
    pub status: StatusResponse,
    #[serde(default)]
    pub workspaces: Vec<LoadedWorkspaceInfo>,
    /// Seconds since this node last heard from the peer (0 for the answering node itself).
    pub last_seen_secs: u64,
    pub alive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClusterResponse {
    /// The answering node's own address.
    pub this_node: String,
    pub nodes: Vec<PeerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaceRequest {
    pub workspace_name: String,
    #[serde(default)]
    pub engine: Option<String>,
    /// The OS the node must run (`macos`), matched against the start of its status platform.
    /// A Go module whose cgo includes macOS headers compiles nowhere else.
    #[serde(default)]
    pub os: Option<String>,
    /// Whether to rebalance workload even if the workspace has active sessions (Phase 5.3).
    #[serde(default)]
    pub rebalance_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaceResponse {
    /// The node to use, or None when no node in the cluster can serve the engine.
    pub node: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricsRequest {
    /// Window in seconds; 0 means everything the node still holds.
    #[serde(default)]
    pub since_secs: u64,
}

/// Queries of one (agent, host, workspace, method) group in the window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryMetric {
    pub agent: String,
    pub host: String,
    pub workspace: String,
    pub method: String,
    pub count: u64,
    pub errors: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
}

/// Commands run through `exec` (including check/lint/test) in the window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecMetric {
    pub agent: String,
    pub host: String,
    pub workspace: String,
    pub command: String,
    pub count: u64,
    pub failures: u64,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricsResponse {
    pub node: String,
    pub since_secs: u64,
    /// Events the node holds in memory (the JSONL files on disk hold everything).
    pub events_in_memory: u64,
    pub queries: Vec<QueryMetric>,
    pub execs: Vec<ExecMetric>,
    /// Sync rounds: files and bytes uploaded to this node in the window.
    pub sync_rounds: u64,
    pub sync_files: u64,
    pub sync_bytes: u64,
}

/// What drives this client, for usage metrics: `PROD_CODE_AGENT` when set, otherwise
/// `claude-code` / `codex` when launched by those tools, else `cli`.
pub fn detect_client_agent() -> String {
    if let Ok(agent) = std::env::var("PROD_CODE_AGENT")
        && !agent.trim().is_empty()
    {
        return agent;
    }
    if std::env::var_os("CLAUDECODE").is_some()
        || std::env::var_os("CLAUDE_CODE_ENTRYPOINT").is_some()
    {
        return "claude-code".to_string();
    }
    if std::env::var_os("CODEX_SANDBOX").is_some()
        || std::env::var_os("CODEX_HOME").is_some()
        || std::env::var_os("CODEX_THREAD_ID").is_some()
    {
        return "codex".to_string();
    }
    "cli".to_string()
}

/// This machine's OS and architecture, as `linux x86_64` or `macos aarch64`.
pub fn platform() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The client machine's hostname (cached).
pub fn client_host() -> String {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|h| !h.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok())
            .unwrap_or_else(|| "unknown".to_string())
    })
    .clone()
}
