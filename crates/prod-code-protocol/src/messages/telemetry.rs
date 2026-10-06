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

/// Normalized operation metric record.
///
/// Captures completed gateway requests and operations with bounded, low-cardinality
/// dimensions: instance/node identity, operation category, method, engine, timing,
/// success/error classification, and resource volume. High-cardinality values
/// (workspace paths, source files, symbols, client IPs, raw commands) are deliberately
/// excluded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationMetric {
    /// Milliseconds since Unix epoch when the operation was recorded.
    pub ts_ms: u64,
    /// Gateway instance/node identity (e.g. advertise address or hostname).
    pub node: String,
    /// Operation category: `lsp`, `exec`, `remote_exec`, `sync`, `search`, `shadow`,
    /// `read_file`, `place`, `status`.
    pub category: String,
    /// Low-cardinality method or toolchain command name (e.g. `textDocument/definition`,
    /// `cargo-check`, `fast-sync`, `search_query`, `shadow_run`).
    pub method: String,
    /// Engine identifier (e.g. `rust`, `go`, `cpp`, `python`, `swift`, `none`).
    pub engine: String,
    /// Total duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the operation completed successfully.
    pub ok: bool,
    /// Normalized low-cardinality error category (e.g. `timeout`, `cancelled`, `invalid_params`,
    /// `exit_non_zero`, `io_error`, `not_found`, `internal_error`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    /// Number of processed items (e.g. hits returned, files synced, references found).
    #[serde(default)]
    pub items: u64,
    /// Number of bytes transferred or produced.
    #[serde(default)]
    pub bytes: u64,
    /// Exit code when running subprocess commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Optional calling agent name (e.g. `codex`, `claude-code`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Optional client hostname.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Optional sanitized workspace identifier (never full local filesystem paths).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Optional compiler name and version (e.g. `rustc 1.85.0`, `go 1.24.0`, `clang 19.1.0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler: Option<String>,
}

impl OperationMetric {
    pub fn new(
        node: impl Into<String>,
        category: impl Into<String>,
        method: impl Into<String>,
    ) -> Self {
        Self {
            ts_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            node: node.into(),
            category: category.into(),
            method: method.into(),
            engine: "none".to_string(),
            duration_ms: 0,
            ok: true,
            error_class: None,
            items: 0,
            bytes: 0,
            exit_code: None,
            agent: None,
            host: None,
            workspace: None,
            compiler: None,
        }
    }
}

/// Periodic host and gateway resource telemetry snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSnapshot {
    /// Milliseconds since Unix epoch when snapshot was taken.
    pub ts_ms: u64,
    /// Gateway instance/node identity.
    pub node: String,
    /// Gateway version string.
    pub version: String,
    /// Git commit hash (or `unknown`).
    pub git_commit: String,
    /// Platform string (e.g. `linux x86_64`, `macos aarch64`).
    pub platform: String,
    /// Logical CPU count.
    pub cpu_count: usize,
    /// Actual CPU usage in thousandths (12340 = 12.34%, 100000 = 100.0%).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_usage_millis: Option<u32>,
    /// 1-minute load average in thousandths (1500 = 1.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_average_millis: Option<u32>,
    /// Gateway process Resident Set Size (RSS) in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_rss_bytes: Option<u64>,
    /// Host memory available without swapping in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_memory_available_bytes: Option<u64>,
    /// Total physical host memory in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_memory_total_bytes: Option<u64>,
    /// Free storage share of the workspaces filesystem in thousandths (150 = 15%).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_free_millis: Option<u32>,
    /// Free storage on workspaces filesystem in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_free_bytes: Option<u64>,
    /// Total storage capacity on workspaces filesystem in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_total_bytes: Option<u64>,
    /// Storage used by workspace directories in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_storage_bytes: Option<u64>,
    /// Count of active client connections / sessions.
    pub active_sessions: usize,
    /// Count of queries currently being executed.
    pub active_queries: usize,
    /// Count of remote commands currently running.
    pub running_commands: usize,
    /// Count of currently loaded workspaces.
    pub workspace_count: usize,
    /// Count of available/advertised engines.
    pub engine_count: usize,
}

impl HostSnapshot {
    pub fn cpu_usage_pct(&self) -> Option<f64> {
        self.cpu_usage_millis.map(|m| m as f64 / 1000.0)
    }

    pub fn load_average_1m(&self) -> Option<f64> {
        self.load_average_millis.map(|m| m as f64 / 1000.0)
    }

    pub fn with_cpu_usage_pct(mut self, pct: f64) -> Self {
        self.cpu_usage_millis = Some((pct * 1000.0).round() as u32);
        self
    }
}

/// A specific toolchain component and its resolved version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolchainVersion {
    pub tool: String,
    pub version: String,
}

/// Detected engine availability and component toolchain inventory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EngineToolchainInfo {
    pub engine: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolchains: Vec<ToolchainVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// Toolchain and engine inventory across the host.
///
/// Collected separately from frequent resource snapshots to avoid running expensive
/// subprocess checks on high-frequency ticks or request paths.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolchainInventory {
    pub ts_ms: u64,
    pub node: String,
    pub engines: Vec<EngineToolchainInfo>,
}

/// Unified telemetry record stream.
///
/// Every line in the gateway telemetry stream (`events-YYYY-MM-DD.jsonl`) serializes
/// as one of these variants, allowing a single normalized consumer/export stream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum TelemetryRecord {
    #[serde(rename = "operation")]
    Operation(OperationMetric),
    #[serde(rename = "snapshot")]
    Snapshot(HostSnapshot),
    #[serde(rename = "inventory")]
    Inventory(ToolchainInventory),
}

impl TelemetryRecord {
    pub fn timestamp_ms(&self) -> u64 {
        match self {
            Self::Operation(op) => op.ts_ms,
            Self::Snapshot(s) => s.ts_ms,
            Self::Inventory(inv) => inv.ts_ms,
        }
    }

    pub fn node(&self) -> &str {
        match self {
            Self::Operation(op) => &op.node,
            Self::Snapshot(s) => &s.node,
            Self::Inventory(inv) => &inv.node,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_record_round_trips() {
        let op = OperationMetric::new("node-1", "lsp", "textDocument/definition");
        let rec = TelemetryRecord::Operation(op.clone());
        let json = serde_json::to_string(&rec).expect("serialize");
        assert!(json.contains("\"type\":\"operation\""));
        assert!(json.contains("\"node\":\"node-1\""));

        let de: TelemetryRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(de, rec);
        assert_eq!(de.node(), "node-1");
    }

    #[test]
    fn host_snapshot_cpu_pct_calculation() {
        let mut snap = HostSnapshot {
            ts_ms: 1000,
            node: "node-a".into(),
            version: "0.3.26".into(),
            git_commit: "deadbeef".into(),
            platform: "linux x86_64".into(),
            cpu_count: 16,
            cpu_usage_millis: Some(25500), // 25.5%
            load_average_millis: Some(1500),
            process_rss_bytes: Some(1024),
            host_memory_available_bytes: Some(2048),
            host_memory_total_bytes: Some(4096),
            storage_free_millis: Some(500),
            storage_free_bytes: Some(8192),
            storage_total_bytes: Some(16384),
            workspace_storage_bytes: Some(1024),
            active_sessions: 2,
            active_queries: 1,
            running_commands: 0,
            workspace_count: 1,
            engine_count: 3,
        };
        assert_eq!(snap.cpu_usage_pct(), Some(25.5));
        assert_eq!(snap.load_average_1m(), Some(1.5));

        snap.cpu_usage_millis = None;
        assert_eq!(snap.cpu_usage_pct(), None);
    }
}
