/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Pruning never takes a copy someone uses: a session on a nested project, a command, or a
//! user who entered after the candidate list was made (#1524).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::workspace::manager::WorkspaceManager;
use crate::workspace::prune::{
    LAST_USED_MARKER, prune_stale_worktree_dirs, prune_worktree_dirs_for_space, touch_last_used,
};
use crate::workspace::usage::{TOMBSTONE_PREFIX, WorkspaceUse};

fn age(dir: &Path, hours: u64) {
    std::fs::File::options()
        .write(true)
        .open(dir.join(LAST_USED_MARKER))
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(hours * 3600))
        .unwrap();
}

fn copy(storage: &Path, name: &str, hours: u64) -> PathBuf {
    let dir = storage.join(name);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/lib.rs"), "x").unwrap();
    touch_last_used(&dir);
    age(&dir, hours);
    dir
}

fn tombstones(storage: &Path) -> usize {
    std::fs::read_dir(storage)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(TOMBSTONE_PREFIX)
        })
        .count()
}

/// A user below the checkout (a nested `engine_subpath` engine) or on it with an old marker
/// keeps the copy; once the use ends, the old copy goes.
#[tokio::test]
async fn a_copy_in_use_at_or_below_its_root_is_not_pruned() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path();
    let manager = WorkspaceManager::new();
    let nested = copy(storage, "repo--wt-00000001", 30 * 24);
    let entered = copy(storage, "repo--wt-00000002", 30 * 24);
    let below = WorkspaceUse::enter(&nested.join("sub"));
    let on_root = WorkspaceUse::enter(&entered);
    // A session that has run for a month has an old marker of its own.
    age(&entered, 30 * 24);
    let week = Duration::from_secs(7 * 86_400);
    assert!(
        prune_stale_worktree_dirs(storage, week, &manager)
            .await
            .is_empty()
    );
    let full = |_: &Path| Some(0.0);
    assert!(
        prune_worktree_dirs_for_space(storage, 0.2, &manager, full)
            .await
            .is_empty()
    );
    assert!(nested.join("sub/lib.rs").is_file() && entered.join("sub/lib.rs").is_file());
    drop((below, on_root));
    let mut removed = prune_stale_worktree_dirs(storage, week, &manager).await;
    removed.sort();
    assert_eq!(removed, [nested, entered]);
    assert_eq!(tombstones(storage), 0, "the moved-aside copies are deleted");
}

/// The space pruner chooses its candidates first. A later candidate that a session enters while
/// an earlier one is being deleted stays: each is checked again just before it goes.
#[tokio::test]
async fn a_candidate_entered_while_an_earlier_one_is_deleted_stays() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path();
    let manager = WorkspaceManager::new();
    let first = copy(storage, "repo--wt-00000001", 44);
    let later = copy(storage, "repo--wt-00000002", 30);
    let calls = Mutex::new(0);
    let entered: Mutex<Option<WorkspaceUse>> = Mutex::new(None);
    // `free` is read once before the loop and again after each deletion: the second read is
    // the moment between deleting `first` and reaching `later`.
    let free = |_: &Path| {
        let mut calls = calls.lock().unwrap();
        *calls += 1;
        if *calls == 2 {
            *entered.lock().unwrap() = Some(WorkspaceUse::enter(&later));
            age(&later, 30);
        }
        Some(0.0)
    };
    let removed = prune_worktree_dirs_for_space(storage, 0.2, &manager, free).await;
    assert_eq!(removed, std::slice::from_ref(&first));
    assert!(!first.exists());
    assert!(
        later.join("sub/lib.rs").is_file(),
        "the entered copy was deleted"
    );
    assert!(entered.lock().unwrap().is_some());
    assert_eq!(tombstones(storage), 0);
}
