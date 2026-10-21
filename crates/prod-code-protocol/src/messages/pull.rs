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

use super::ReadFileResponse;

/// Artifact transfers are independent of analyzer sessions and bounded per frame.
/// Base64 expansion of this chunk remains well below the transport frame limit.
pub const PULL_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullFileRequest {
    pub client_workspace_root: String,
    pub base_workspace_name: Option<String>,
    /// Relative path inside this checkout's copy, including ignored build outputs.
    pub path: String,
    pub offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullFileResponse {
    pub server_workspace_root: String,
    pub offset: u64,
    pub total_bytes: u64,
    /// `truncated` means more chunks remain, rather than a transfer size limit.
    pub file: ReadFileResponse,
}
