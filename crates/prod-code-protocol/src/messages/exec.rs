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

use super::sync::{FileDelta, base64_bytes};

/// Run `command` (argv, no shell) in the server workspace that mirrors the client's checkout.
/// Build artifacts (`target/`, `node_modules/`) stay on the server between runs, so every
/// worktree keeps its own warm cache.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecRequest {
    pub client_workspace_root: String,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
    pub command: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Kill the command after this many seconds; 0 means the server default.
    #[serde(default)]
    pub timeout_secs: u64,
    /// After the command, send back files it created, changed or deleted (`ExecChanges`), so
    /// formatters, code generators and lockfile updates land in the client's checkout.
    #[serde(default)]
    pub pull_changes: bool,
    /// Directory inside the workspace (relative, `/`-separated) to run the command in; the
    /// workspace root when absent. Lets a nested project be built and tested in place.
    #[serde(default)]
    pub subdir: Option<String>,
    #[serde(default)]
    pub client_agent: Option<String>,
    #[serde(default)]
    pub client_host: Option<String>,
}

/// Files the command changed in the server workspace, sent before `ExecExit` when
/// `ExecRequest::pull_changes` was set. Deletions carry `content: None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecChanges {
    pub files: Vec<FileDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecChunk {
    pub stderr: bool,
    /// Raw bytes, base64 (output may be partial UTF-8 or contain terminal escapes).
    #[serde(with = "base64_bytes")]
    pub data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecExit {
    /// Process exit code; None when killed by a signal or by the timeout.
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub server_workspace_root: String,
    #[serde(default)]
    pub timed_out: bool,
    /// Set when the command could not be started at all.
    #[serde(default)]
    pub error: Option<String>,
    /// What the command and every descendant it waited for used, when the server could tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ExecUsage>,
    /// The OS and architecture the command ran on (`linux x86_64`), from [`platform`]. A fix or a
    /// lint computed there is for that platform, whatever the checkout targets (#140).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

/// Resource use of a finished command, from `wait4`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecUsage {
    pub cpu_user_ms: u64,
    pub cpu_sys_ms: u64,
    /// Peak resident set size of the largest process in the tree, in KiB.
    pub max_rss_kb: u64,
}

impl ExecUsage {
    /// `cpu 12.3s user 1.2s sys, peak 512 MB`.
    pub fn render(&self) -> String {
        format!(
            "cpu {:.1}s user {:.1}s sys, peak {} MB",
            self.cpu_user_ms as f64 / 1000.0,
            self.cpu_sys_ms as f64 / 1000.0,
            self.max_rss_kb.div_ceil(1024)
        )
    }
}
