/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// Unique identifier for a shared workspace based on its canonical root.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceKey(pub PathBuf);

/// Every in-memory Rust engine that answers for one workspace root: the main one, and the
/// validation engine once a validation session has loaded it. A file that changes on disk must
/// reach all of them, and the sync paths only ever see one [`SharedWorkspace`], so the list is
/// shared between the workspace and the validation view derived from it.
pub type RustEngines = Arc<std::sync::Mutex<Vec<Arc<Mutex<prod_code_engine_rust::RustEngine>>>>>;

/// Loads the in-process Rust engine of a root, on a blocking thread; tests put a slow one in
/// its place.
pub type RustLoader = Arc<dyn Fn(&Path) -> Result<prod_code_engine_rust::RustEngine> + Send + Sync>;

/// How a sync changed a file on disk, numbered as LSP's `FileChangeType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchedChange {
    Created = 1,
    Changed = 2,
    Deleted = 3,
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Default bound on concurrent cold engine loads (#408).
/// On high-core nodes (e.g. 128 cores), loading 16 engines simultaneously starves CPU and I/O caches
/// and drives first-query response times past timeouts. Limiting in-flight loads ensures the first
/// workspaces load quickly and answer within their budget.
pub fn default_max_concurrent_engine_loads() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    if cpus >= 32 {
        8
    } else if cpus >= 8 {
        4
    } else {
        2
    }
}
