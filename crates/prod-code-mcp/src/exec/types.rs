/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::UploadExclusion;
use prod_code_protocol::{ExecExit, RemoteExecResult};
use serde::{Deserialize, Serialize};

/// Where a command actually ran (#1793): the gateway it was sent to and, under a strict pin, the
/// node that gateway confirmed on the same connection that it executes the work on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Executor {
    /// The gateway address the command ran on.
    pub node: String,
    /// The node the gateway confirmed, when the workspace is strictly pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_node: Option<String>,
}

impl Executor {
    pub fn new(node: std::net::SocketAddr, confirmed_node: Option<String>) -> Self {
        Self {
            node: node.to_string(),
            confirmed_node,
        }
    }

    /// `node A` or `node A (gateway confirms B)`.
    pub fn render(&self) -> String {
        match &self.confirmed_node {
            Some(confirmed) => format!("node {} (gateway confirms {confirmed})", self.node),
            None => format!("node {}", self.node),
        }
    }
}

/// Result of a remote command: its exit record plus the checkout files the command changed on
/// the server and that were written back locally.
#[derive(Debug, Clone)]
pub struct RemoteOutcome {
    /// The node the command ran on.
    pub executor: Executor,
    pub exit: ExecExit,
    pub pulled_files: Vec<String>,
    /// Rust files rewritten by a known formatter with proven ordered whitespace equivalence.
    pub relaid_files: Vec<String>,
    /// Returned paths whose local state changed or could not be established. Their local
    /// state, including absence, is kept and the node's version is not written (#254).
    pub kept_files: Vec<String>,
    /// Written files with platform evidence in either the pre-write or returned bytes.
    /// Retained after deletion or removal of the last platform marker.
    pub platform_files: Vec<String>,
    /// Unignored local checkout files omitted during pre-flight sync (#1836).
    pub sync_omitted: Vec<UploadExclusion>,
    /// Written-back files the next upload leaves out under the ordinary sync rules, so
    /// another node's copy never sees them (#1813).
    pub upload_excluded: Vec<UploadExclusion>,
}

impl RemoteOutcome {
    pub fn platform_warning(&self, root: &std::path::Path) -> Option<String> {
        super::platform::platform_warning_with_evidence(
            root,
            self.exit.platform.as_deref(),
            &self.changed_code(),
            &self.platform_files,
        )
    }

    /// Compact warning naming [`Self::sync_omitted`] and `[sync] include` (#1836).
    pub fn sync_warning(&self) -> Option<String> {
        crate::sync::sync_omission_warning(&self.sync_omitted)
    }

    /// The compact warning naming [`Self::upload_excluded`] and the portable alternatives.
    pub fn upload_warning(&self) -> Option<String> {
        crate::sync::upload_exclusion_warning(&self.upload_excluded)
    }

    /// The written-back files whose code changed, not only its layout.
    pub fn changed_code(&self) -> Vec<String> {
        self.pulled_files
            .iter()
            .filter(|f| !self.relaid_files.contains(f))
            .cloned()
            .collect()
    }
}

/// Result of a polyglot remote command execution including structured results.
#[derive(Debug, Clone)]
pub struct PolyglotRemoteOutcome {
    /// The node the command ran on.
    pub executor: Executor,
    pub result: RemoteExecResult,
    pub pulled_files: Vec<String>,
    pub relaid_files: Vec<String>,
    pub kept_files: Vec<String>,
    pub platform_files: Vec<String>,
}

impl PolyglotRemoteOutcome {
    pub fn platform_warning(&self, root: &std::path::Path) -> Option<String> {
        super::platform::platform_warning_with_evidence(
            root,
            self.result.platform.as_deref(),
            &self.changed_code(),
            &self.platform_files,
        )
    }

    pub fn changed_code(&self) -> Vec<String> {
        self.pulled_files
            .iter()
            .filter(|f| !self.relaid_files.contains(f))
            .cloned()
            .collect()
    }
}

/// Keeps the last `limit` bytes of combined output for a compact tool result.
pub struct TailBuffer {
    limit: usize,
    buf: Vec<u8>,
    pub total: usize,
}

impl TailBuffer {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            buf: Vec::new(),
            total: 0,
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.total += data.len();
        self.buf.extend_from_slice(data);
        if self.buf.len() > self.limit {
            let cut = self.buf.len() - self.limit;
            self.buf.drain(..cut);
        }
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }
}
