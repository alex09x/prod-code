/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::index::SearchIndexes;
use super::types::DEFAULT_LIMIT;
use prod_code_protocol::{SearchRequest, SearchResponse};
use std::path::Path;
use std::time::Instant;

/// Normalizes a wire path without host path semantics, so every gateway rejects the same
/// absolute, drive and parent-traversal forms. Empty and dot-only paths mean the workspace root.
pub(crate) fn normalize_subpath(raw: Option<&str>) -> Result<Option<String>, &'static str> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let bytes = raw.as_bytes();
    if raw.starts_with(['/', '\\']) {
        return Err("must be relative");
    }
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err("must not use a drive path");
    }
    let mut components = Vec::new();
    for component in raw.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => return Err("must not contain parent traversal"),
            component => components.push(component),
        }
    }
    Ok((!components.is_empty()).then(|| components.join("/")))
}

/// Answers a `SearchRequest` against the workspace copy.
pub fn run_search(
    indexes: &SearchIndexes,
    storage_root: &Path,
    req: &SearchRequest,
) -> SearchResponse {
    let workspace = crate::workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let workspace_str = workspace.to_string_lossy().to_string();
    if !workspace.is_dir() {
        return SearchResponse {
            server_workspace_root: workspace_str.clone(),
            hits: Vec::new(),
            indexed_files: 0,
            indexed_declarations: 0,
            took_ms: 0,
            error: Some(format!(
                "workspace {workspace_str} is not synced to this gateway"
            )),
            dense: None,
            graph_fused: None,
        };
    }
    if req.query.trim().is_empty() {
        return SearchResponse {
            server_workspace_root: workspace_str,
            hits: Vec::new(),
            indexed_files: 0,
            indexed_declarations: 0,
            took_ms: 0,
            error: Some("empty query".to_string()),
            dense: None,
            graph_fused: None,
        };
    }
    let started = Instant::now();
    let limit = if req.limit == 0 {
        DEFAULT_LIMIT
    } else {
        req.limit
    };
    let subpath = match normalize_subpath(req.subpath.as_deref()) {
        Ok(subpath) => subpath,
        Err(reason) => {
            return SearchResponse {
                server_workspace_root: workspace_str,
                hits: Vec::new(),
                indexed_files: 0,
                indexed_declarations: 0,
                took_ms: started.elapsed().as_millis() as u64,
                error: Some(format!(
                    "invalid search subpath {:?}: {reason}",
                    req.subpath
                )),
                dense: None,
                graph_fused: None,
            };
        }
    };
    let found = indexes.search(&workspace, &req.query, limit, subpath.as_deref());
    SearchResponse {
        server_workspace_root: workspace_str,
        hits: found.hits,
        indexed_files: found.files,
        indexed_declarations: found.declarations,
        took_ms: started.elapsed().as_millis() as u64,
        error: None,
        dense: found.dense,
        graph_fused: Some(found.graph_fused),
    }
}
