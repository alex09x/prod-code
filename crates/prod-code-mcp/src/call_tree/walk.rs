/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashSet;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::time::Instant;

use anyhow::Result;

use super::cache::{
    CALL_CACHE, CallCacheEntry, MAX_DEPTH, MAX_NODES, PREPARE_CACHE, PrepareCacheEntry,
    normalize_root,
};
use super::types::{
    CallTree, Node, key_of, node_of, valid_lsp_range, validate_call_hierarchy_item,
};
use crate::tools::execute_lsp_query;

struct Walk<'a> {
    remote: SocketAddr,
    root: &'a Path,
    file: &'a Path,
    incoming: bool,
    depth: usize,
    generation: u64,
    seen: HashSet<(String, u64, u64)>,
    shown: usize,
    truncated: bool,
}

impl Walk<'_> {
    /// The functions one level below `item`, each expanded while the depth and budget last.
    fn expand(
        &mut self,
        item: serde_json::Value,
        level: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Node>>> + Send + '_>> {
        Box::pin(async move {
            let method = if self.incoming {
                "callHierarchy/incomingCalls"
            } else {
                "callHierarchy/outgoingCalls"
            };
            let side = if self.incoming { "from" } else { "to" };
            let (key_uri, key_line, key_col) = key_of(&item);
            let root_str = normalize_root(self.root);
            let cache_key = (
                self.remote,
                root_str,
                key_uri,
                key_line,
                key_col,
                self.incoming,
                self.generation,
            );
            let cached_edges = {
                let mut lock = CALL_CACHE.lock().unwrap_or_else(|e| e.into_inner());
                if lock.len() > 2048 {
                    lock.clear();
                }
                lock.get(&cache_key)
                    .filter(|e| e.timestamp.elapsed().as_secs() < 60)
                    .map(|e| e.edges.clone())
            };
            let (edges, from_cache) = match cached_edges {
                Some(edges) => (edges, true),
                None => {
                    let res = execute_lsp_query(
                        self.remote,
                        self.root,
                        self.file,
                        method,
                        serde_json::json!({ "item": item }),
                    )
                    .await?;
                    (res, false)
                }
            };
            let edges_array = match &edges {
                serde_json::Value::Null => {
                    if !from_cache {
                        let mut lock = CALL_CACHE.lock().unwrap_or_else(|e| e.into_inner());
                        lock.insert(
                            cache_key,
                            CallCacheEntry {
                                edges: edges.clone(),
                                timestamp: Instant::now(),
                            },
                        );
                    }
                    return Ok(Vec::new());
                }
                serde_json::Value::Array(a) => a,
                other => {
                    anyhow::bail!("the analyzer's {method} answer is not an array or null: {other}")
                }
            };
            for edge in edges_array {
                anyhow::ensure!(
                    edge.is_object(),
                    "call hierarchy edge in {method} is not an object: {edge}"
                );
                let Some(other) = edge.get(side) else {
                    anyhow::bail!("call hierarchy edge in {method} is missing '{side}': {edge}");
                };
                validate_call_hierarchy_item(other)?;
                let Some(ranges) = edge.get("fromRanges").and_then(|v| v.as_array()) else {
                    anyhow::bail!(
                        "call hierarchy edge in {method} has no valid 'fromRanges' array: {edge}"
                    );
                };
                for range in ranges {
                    anyhow::ensure!(
                        valid_lsp_range(range),
                        "call hierarchy edge 'fromRanges' entry in {method} is not a valid range: {range}"
                    );
                }
            }
            if !from_cache {
                let mut lock = CALL_CACHE.lock().unwrap_or_else(|e| e.into_inner());
                lock.insert(
                    cache_key,
                    CallCacheEntry {
                        edges: edges.clone(),
                        timestamp: Instant::now(),
                    },
                );
            }
            let mut nodes = Vec::new();
            for edge in edges_array {
                let other = edge.get(side).unwrap();
                if self.shown >= MAX_NODES {
                    self.truncated = true;
                    break;
                }
                self.shown += 1;
                let mut node = node_of(edge, other);
                if !self.seen.insert(key_of(other)) {
                    node.repeated = true;
                } else if level < self.depth {
                    node.children = self.expand(other.clone(), level + 1).await?;
                }
                nodes.push(node);
            }
            Ok(nodes)
        })
    }
}

/// The callers (`incoming`) or callees of the function at the 1-based `line`:`character` of
/// `file`, to `depth` levels (1 is the direct ones; at most [`MAX_DEPTH`]). `None` when there is
/// no function there.
pub async fn call_tree(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    incoming: bool,
    depth: usize,
) -> Result<Option<CallTree>> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {file:?}"))?
        .to_string();
    let generation = crate::watch::current_generation(root);
    let root_str = normalize_root(root);
    let prepare_key = (remote, root_str, uri.clone(), line, character, generation);
    let cached_items = {
        let mut lock = PREPARE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if lock.len() > 1024 {
            lock.clear();
        }
        lock.get(&prepare_key)
            .filter(|e| e.timestamp.elapsed().as_secs() < 60)
            .map(|e| e.items.clone())
    };
    let (items, from_cache) = match cached_items {
        Some(items) => (items, true),
        None => {
            let res = execute_lsp_query(
                remote,
                root,
                file,
                "textDocument/prepareCallHierarchy",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
                }),
            )
            .await?;
            (res, false)
        }
    };
    let items_array = match &items {
        serde_json::Value::Null => {
            if !from_cache {
                let mut lock = PREPARE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
                lock.insert(
                    prepare_key,
                    PrepareCacheEntry {
                        items: items.clone(),
                        timestamp: Instant::now(),
                    },
                );
            }
            return Ok(None);
        }
        serde_json::Value::Array(a) => a,
        other => anyhow::bail!(
            "the analyzer's prepareCallHierarchy answer is not an array or null: {other}"
        ),
    };
    if items_array.is_empty() {
        if !from_cache {
            let mut lock = PREPARE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            lock.insert(
                prepare_key,
                PrepareCacheEntry {
                    items: items.clone(),
                    timestamp: Instant::now(),
                },
            );
        }
        return Ok(None);
    }
    for item in items_array {
        validate_call_hierarchy_item(item)?;
    }
    if !from_cache {
        let mut lock = PREPARE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        lock.insert(
            prepare_key,
            PrepareCacheEntry {
                items: items.clone(),
                timestamp: Instant::now(),
            },
        );
    }
    let item = items_array[0].clone();
    let depth = depth.clamp(1, MAX_DEPTH);
    let mut walk = Walk {
        remote,
        root,
        file,
        incoming,
        depth,
        generation,
        seen: HashSet::from([key_of(&item)]),
        shown: 0,
        truncated: false,
    };
    let nodes = walk.expand(item.clone(), 1).await?;
    Ok(Some(CallTree {
        name: item
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("?")
            .to_string(),
        incoming,
        depth,
        nodes,
        truncated: walk.truncated,
    }))
}
