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
/// Maximum number of directory entries returned in one read-only listing.
pub const ARTIFACT_LIST_LIMIT: usize = 1000;

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullFileRequest {
    pub client_workspace_root: String,
    pub base_workspace_name: Option<String>,
    /// Relative path inside this checkout's copy, including ignored build outputs.
    pub path: String,
    pub offset: u64,
    /// List a directory instead of transferring file contents. Older gateways ignore this
    /// field and refuse the directory, rather than writing anything locally.
    #[serde(default, skip_serializing_if = "is_false")]
    pub list_directory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactEntry {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactListing {
    pub entries: Vec<ArtifactEntry>,
    /// Some entries were omitted because the limit or UTF-8 representation was exceeded.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullFileResponse {
    pub server_workspace_root: String,
    pub offset: u64,
    pub total_bytes: u64,
    /// `truncated` means more chunks remain, rather than a transfer size limit.
    pub file: ReadFileResponse,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_pull_requests_default_to_file_transfer() {
        let old = serde_json::json!({"client_workspace_root":"/workspace", "base_workspace_name":null, "path":"a", "offset":0});
        let request: PullFileRequest = serde_json::from_value(old).unwrap();
        assert!(!request.list_directory);
        assert!(
            serde_json::to_value(request)
                .unwrap()
                .get("list_directory")
                .is_none()
        );
    }
}
