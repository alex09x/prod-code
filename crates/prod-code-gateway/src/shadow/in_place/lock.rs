/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub(crate) type InPlaceLocks =
    Mutex<std::collections::HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>;

/// One lock per canonical workspace path, shared by every request: in-place hypotheses write
/// into the workspace copy itself, so two of them must never overlap. An entry lives as long
/// as someone holds or waits for its lock.
pub(crate) fn in_place_locks() -> &'static InPlaceLocks {
    static LOCKS: OnceLock<InPlaceLocks> = OnceLock::new();
    LOCKS.get_or_init(Default::default)
}

pub(crate) fn in_place_key(workspace: &Path) -> PathBuf {
    std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf())
}

pub(crate) fn in_place_lock(key: &Path) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = in_place_locks().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
        return lock;
    }
    // Entries a cancelled waiter left behind go here, so the map holds only live locks.
    locks.retain(|_, lock| lock.strong_count() > 0);
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(key.to_path_buf(), Arc::downgrade(&lock));
    lock
}

pub(crate) fn forget_unused_lock(key: &Path) {
    let mut locks = in_place_locks().lock().unwrap_or_else(|e| e.into_inner());
    if locks.get(key).is_some_and(|lock| lock.strong_count() == 0) {
        locks.remove(key);
    }
}

/// The right to change one workspace in place; the lock entry goes with the last user.
pub(crate) struct WorkspaceTurn {
    key: PathBuf,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl WorkspaceTurn {
    /// Waits for the workspace's lock, or `None` when the client leaves first.
    pub(crate) async fn wait(
        workspace: &Path,
        cancel: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Option<Self> {
        let key = in_place_key(workspace);
        let lock = in_place_lock(&key);
        let guard = async {
            let acquire = lock.lock_owned();
            tokio::pin!(acquire);
            loop {
                if *cancel.borrow_and_update() {
                    return None;
                }
                tokio::select! {
                    guard = &mut acquire => return Some(guard),
                    changed = cancel.changed() => if changed.is_err() {
                        // Nobody can cancel any more.
                        return Some(acquire.await);
                    },
                }
            }
        }
        .await;
        match guard {
            Some(guard) => Some(Self {
                key,
                guard: Some(guard),
            }),
            None => {
                forget_unused_lock(&key);
                None
            }
        }
    }
}

impl Drop for WorkspaceTurn {
    fn drop(&mut self) {
        drop(self.guard.take());
        forget_unused_lock(&self.key);
    }
}
