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

#[derive(Default, Clone)]
struct WorkspaceLocks {
    mutation: Arc<tokio::sync::RwLock<()>>,
    writer: Arc<tokio::sync::Mutex<()>>,
}

static LOCKS: LazyLock<Mutex<HashMap<PathBuf, WorkspaceLocks>>> = LazyLock::new(Default::default);

pub(super) enum WorkspaceMutationGuard {
    Read(#[allow(dead_code)] tokio::sync::OwnedRwLockReadGuard<()>),
    Write(#[allow(dead_code)] tokio::sync::OwnedRwLockWriteGuard<()>),
}

fn workspace_locks(root: &Path) -> WorkspaceLocks {
    let key = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key)
        .or_default()
        .clone()
}

pub(super) fn workspace_writer_gate(root: &Path) -> Arc<tokio::sync::Mutex<()>> {
    workspace_locks(root).writer
}

pub(crate) fn workspace_mutation_lock(root: &Path) -> Arc<tokio::sync::RwLock<()>> {
    workspace_locks(root).mutation
}
