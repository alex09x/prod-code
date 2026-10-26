/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::Result;

use super::cache::{
    CALL_CACHE, CallCacheEntry, DEFAULT_CALL_TREE_BUDGET, MAX_DEPTH, MAX_NODES, PREPARE_CACHE,
    PrepareCacheEntry, normalize_root,
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
    started: Instant,
    deadline: tokio::time::Instant,
    /// Every function shown, with the shallowest level it was shown at: a later path that
    /// reaches it higher up still has depth left to expand it (#1389).
    seen: HashMap<(String, u64, u64), usize>,
    shown: usize,
    truncated: bool,
    timed_out_at_depth: Option<usize>,
}

impl Walk<'_> {
    fn record_timeout(&mut self, completed_depth: usize) {
        let clamped = completed_depth.min(self.depth);
        self.timed_out_at_depth = Some(
            self.timed_out_at_depth
                .map_or(clamped, |prev| prev.min(clamped)),
        );
    }

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
                    if tokio::time::Instant::now() >= self.deadline {
                        self.record_timeout(level.saturating_sub(1));
                        return Ok(Vec::new());
                    }
                    let query_fut = execute_lsp_query(
                        self.remote,
                        self.root,
                        self.file,
                        method,
                        serde_json::json!({ "item": item }),
                    );
                    match tokio::time::timeout_at(self.deadline, query_fut).await {
                        Ok(res) => (res?, false),
                        Err(_) => {
                            self.record_timeout(level.saturating_sub(1));
                            return Ok(Vec::new());
                        }
                    }
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
            // Pass 1: materialize all sibling nodes at `level` before expanding `level + 1`
            // so that a time-budget cutoff during deeper expansion preserves the full current level.
            let mut nodes = Vec::new();
            let mut to_expand = Vec::new();
            for edge in edges_array {
                let other = edge.get(side).unwrap();
                if self.shown >= MAX_NODES {
                    self.truncated = true;
                    break;
                }
                self.shown += 1;
                let mut node = node_of(edge, other);
                let shallowest = self.seen.entry(key_of(other)).or_insert(usize::MAX);
                if *shallowest <= level {
                    // Shown already with at least as much depth left; a cycle lands here too.
                    node.repeated = true;
                } else {
                    *shallowest = level;
                    if level < self.depth {
                        to_expand.push((nodes.len(), other.clone()));
                    }
                }
                nodes.push(node);
            }
            // Pass 2: expand deeper levels while within the wall-clock budget.
            let total_to_expand = to_expand.len();
            for (pos, (idx, child_item)) in to_expand.into_iter().enumerate() {
                if tokio::time::Instant::now() >= self.deadline {
                    self.record_timeout(level);
                    break;
                }
                nodes[idx].children = self.expand(child_item, level + 1).await?;
                if self.timed_out_at_depth.is_some() && tokio::time::Instant::now() >= self.deadline
                {
                    if pos + 1 < total_to_expand {
                        self.record_timeout(level);
                    }
                    break;
                }
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
    call_tree_with_budget(
        remote,
        root,
        file,
        line,
        character,
        incoming,
        depth,
        DEFAULT_CALL_TREE_BUDGET,
    )
    .await
}

/// Like [`call_tree`], with an explicit server-side wall-clock budget (#1124).
#[allow(clippy::too_many_arguments)]
pub async fn call_tree_with_budget(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    incoming: bool,
    depth: usize,
    budget: Duration,
) -> Result<Option<CallTree>> {
    let started = Instant::now();
    let deadline = tokio::time::Instant::now() + budget;
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
        started,
        deadline,
        seen: HashMap::from([(key_of(&item), 0)]),
        shown: 0,
        truncated: false,
        timed_out_at_depth: None,
    };
    let nodes = walk.expand(item.clone(), 1).await?;
    let timed_out_at_depth = walk
        .timed_out_at_depth
        .map(|d| (d, walk.started.elapsed().as_secs_f64()));
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
        timed_out_at_depth,
    }))
}
