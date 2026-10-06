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
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

/// The deepest tree asked for; a larger depth is read as this.
pub const MAX_DEPTH: usize = 6;
/// Functions shown in one tree, over every level.
pub const MAX_NODES: usize = 300;

pub(crate) fn normalize_root(root: &Path) -> String {
    std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

pub(crate) type CallCacheKey = (SocketAddr, String, String, u64, u64, bool, u64); // (remote, root, uri, line, col, incoming, generation)

#[derive(Clone)]
pub(crate) struct CallCacheEntry {
    pub(crate) edges: serde_json::Value,
    pub(crate) timestamp: Instant,
}

pub(crate) static CALL_CACHE: LazyLock<Mutex<HashMap<CallCacheKey, CallCacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) type PrepareCacheKey = (SocketAddr, String, String, u32, u32, u64); // (remote, root, uri, line, character, generation)

#[derive(Clone)]
pub(crate) struct PrepareCacheEntry {
    pub(crate) items: serde_json::Value,
    pub(crate) timestamp: Instant,
}

pub(crate) static PREPARE_CACHE: LazyLock<Mutex<HashMap<PrepareCacheKey, PrepareCacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Clear in-memory call hierarchy caches (called on workspace changes or in tests).
pub fn clear_call_hierarchy_cache() {
    if let Ok(mut lock) = CALL_CACHE.lock() {
        lock.clear();
    }
    if let Ok(mut lock) = PREPARE_CACHE.lock() {
        lock.clear();
    }
}

/// Clear in-memory call hierarchy caches for the specified workspace root.
pub fn clear_call_hierarchy_cache_for(root: &Path) {
    let root_str = normalize_root(root);
    let raw_str = root.to_string_lossy();
    if let Ok(mut lock) = CALL_CACHE.lock() {
        lock.retain(|(_, r, ..), _| r != &root_str && r != raw_str.as_ref());
    }
    if let Ok(mut lock) = PREPARE_CACHE.lock() {
        lock.retain(|(_, r, ..), _| r != &root_str && r != raw_str.as_ref());
    }
}
