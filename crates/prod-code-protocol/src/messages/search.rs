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

/// Search a workspace's declarations by intent (roadmap 8.4): the gateway keeps an index of
/// every declaration with the doc comment above it, and ranks them against the query's words.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchRequest {
    pub client_workspace_root: String,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
    pub query: String,
    /// Hits to return; 0 means the server default.
    #[serde(default)]
    pub limit: usize,
    /// Restrict to declarations under this relative path.
    #[serde(default)]
    pub subpath: Option<String>,
    #[serde(default)]
    pub client_agent: Option<String>,
    #[serde(default)]
    pub client_host: Option<String>,
}

/// One declaration the query matched.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchHit {
    /// Path relative to the workspace root.
    pub file: String,
    pub line: u32,
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub container: Option<String>,
    pub signature: String,
    /// First sentence of the doc comment attached to the declaration.
    #[serde(default)]
    pub doc: String,
    /// Attributable ranking score formatted as a string (e.g. "0.0345").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<String>,
    /// Attributable ranking breakdown reasons (e.g. lexical BM25, typed graph centrality, dense similarity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_reasons: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResponse {
    pub server_workspace_root: String,
    pub hits: Vec<SearchHit>,
    pub indexed_files: usize,
    pub indexed_declarations: usize,
    pub took_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
    /// The dense half of the ranking; `None` when the gateway has no embedding model and the
    /// ranking was lexical only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dense: Option<DenseStatus>,
    /// Whether typed graph fusion was applied during search ranking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_fused: Option<bool>,
}

/// How far the dense half of a search had got.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct DenseStatus {
    /// The question was ranked by meaning too: the model is there and some declarations have
    /// vectors.
    pub used: bool,
    /// Declarations with a vector so far; the rest are being embedded in the background.
    pub embedded: usize,
}
