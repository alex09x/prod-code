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

/// One hypothesis of a shadow run (roadmap 7.4): a name and the complete contents of the files
/// it changes, relative to the workspace root; `content: None` deletes the file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShadowHypothesis {
    pub name: String,
    pub files: Vec<FileDelta>,
}

/// Run `command` once per hypothesis, each in its own shadow of the server workspace. On Linux
/// a shadow is an overlay mount of the workspace copy at the workspace's own path: build tools
/// see the same absolute paths, so warm caches (`target/`, `node_modules/`) stay valid, every
/// write lands in the hypothesis's upper directory and the workspace copy is never modified;
/// hypotheses run in parallel. Without user namespaces they run one at a time in place and the
/// touched files are restored afterwards.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShadowRunRequest {
    pub client_workspace_root: String,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
    pub hypotheses: Vec<ShadowHypothesis>,
    pub command: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Kill a hypothesis after this many seconds; 0 means the server default.
    #[serde(default)]
    pub timeout_secs: u64,
    /// Directory inside the workspace (relative, `/`-separated) to run in; the root when absent.
    #[serde(default)]
    pub subdir: Option<String>,
    /// Hypotheses run at once; 0 means the server default (cores / 8, at least 1).
    #[serde(default)]
    pub parallel: usize,
    /// Bytes of combined output kept per hypothesis (its tail); 0 means the server default.
    #[serde(default)]
    pub tail_bytes: usize,
    /// Run hypotheses in a lightweight RAM-backed (/dev/shm) in-memory overlay shadow root (Phase 7.4).
    #[serde(default)]
    pub in_memory: bool,
    #[serde(default)]
    pub client_agent: Option<String>,
    #[serde(default)]
    pub client_host: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShadowHypothesisResult {
    pub name: String,
    /// Process exit code; None when killed by a signal, the timeout or a client disconnect.
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    #[serde(default)]
    pub timed_out: bool,
    /// Set when the hypothesis could not be staged or started at all.
    #[serde(default)]
    pub error: Option<String>,
    /// Tail of the combined stdout/stderr, base64 on the wire.
    #[serde(default, with = "base64_bytes")]
    pub output_tail: Option<Vec<u8>>,
    /// Bytes the command printed in total.
    #[serde(default)]
    pub output_len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShadowRunResponse {
    pub server_workspace_root: String,
    /// `overlay` (user namespace + overlayfs, hypotheses in parallel) or `in-place`
    /// (one at a time, files restored afterwards).
    pub mode: String,
    pub results: Vec<ShadowHypothesisResult>,
    /// Set when the run could not start at all (workspace not synced, empty command, ...).
    #[serde(default)]
    pub error: Option<String>,
}
