/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Who uses a workspace copy right now, and the one step that may take a copy away from them.
//!
//! A session (an engine, a nested `engine_subpath` engine, a dedicated editor) and a remote
//! command hold a [`WorkspaceUse`] on their checkout for as long as they run. The pruners take
//! a copy only through [`claim_for_removal`], which checks for users and for a fresh last-used
//! marker under the same lock that entering takes, and moves the copy aside in that same step.
//! A user therefore either enters first and keeps the copy, or finds it gone and resyncs a
//! fresh one; no deletion runs on a copy someone entered after it was chosen (#1524).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

use super::prune::touch_last_used;

/// The prefix of a copy moved aside for deletion; hidden, so no pruner or sync takes it for a
/// workspace.
pub(crate) const TOMBSTONE_PREFIX: &str = ".prod-code-pruning-";

static USERS: LazyLock<Mutex<HashMap<PathBuf, usize>>> = LazyLock::new(Default::default);

fn users() -> MutexGuard<'static, HashMap<PathBuf, usize>> {
    USERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// A use of the workspace copy at a path, from [`WorkspaceUse::enter`] until it drops.
pub struct WorkspaceUse {
    path: PathBuf,
}

impl WorkspaceUse {
    /// Marks `path` used and records the use on its last-used marker, both under the lock
    /// [`claim_for_removal`] takes. A copy that was already moved aside stays gone: the caller
    /// checks for the directory after entering.
    pub fn enter(path: &Path) -> Self {
        let mut users = users();
        *users.entry(path.to_path_buf()).or_default() += 1;
        touch_last_used(path);
        Self {
            path: path.to_path_buf(),
        }
    }
}

impl Drop for WorkspaceUse {
    fn drop(&mut self) {
        let mut users = users();
        if let Some(count) = users.get_mut(&self.path) {
            *count -= 1;
            if *count == 0 {
                users.remove(&self.path);
            }
        }
    }
}

/// Whether anyone uses `path` or a directory below it.
pub fn in_use_under(path: &Path) -> bool {
    users().keys().any(|used| used.starts_with(path))
}

/// Moves `path` aside for deletion when nobody uses it or anything below it and `still_idle`
/// holds for it, and returns where it went. The check and the rename happen under the lock
/// [`WorkspaceUse::enter`] takes. The caller deletes the returned directory.
pub(crate) fn claim_for_removal(
    path: &Path,
    still_idle: impl FnOnce(&Path) -> bool,
) -> Option<PathBuf> {
    let users = users();
    if users.keys().any(|used| used.starts_with(path)) || !still_idle(path) {
        return None;
    }
    let name = path.file_name()?.to_string_lossy();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tombstone = path.with_file_name(format!("{TOMBSTONE_PREFIX}{name}-{nanos}"));
    std::fs::rename(path, &tombstone).ok()?;
    drop(users);
    Some(tombstone)
}

/// Deletes the copies an earlier prune moved aside but did not finish deleting.
pub(crate) fn remove_leftover_tombstones(storage_root: &Path) {
    let Ok(entries) = std::fs::read_dir(storage_root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(TOMBSTONE_PREFIX)
            && entry.file_type().is_ok_and(|t| t.is_dir())
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}
