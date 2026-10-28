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
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::{Mutex as AsyncMutex, RwLock};

static WORKSPACE_MUTATION_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<RwLock<()>>>>> =
    LazyLock::new(Default::default);
static WORKSPACE_EXEC_LOCKS: LazyLock<Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>> =
    LazyLock::new(Default::default);

fn workspace_key(workspace_root: &Path) -> PathBuf {
    std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf())
}

pub(crate) fn workspace_mutation_lock(workspace_root: &Path) -> Arc<RwLock<()>> {
    WORKSPACE_MUTATION_LOCKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entry(workspace_key(workspace_root))
        .or_insert_with(|| Arc::new(RwLock::new(())))
        .clone()
}

/// Serializes commands that share a remote workspace copy without blocking analyzer reads.
pub(crate) fn workspace_exec_lock(workspace_root: &Path) -> Arc<AsyncMutex<()>> {
    WORKSPACE_EXEC_LOCKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entry(workspace_key(workspace_root))
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}
