/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::workspace::manager::WorkspaceManager;
use crate::workspace::paths::sanitize_identifier;
use crate::workspace::prune::{
    LAST_USED_MARKER, free_share, prune_stale_main_workspace_dirs, prune_stale_worktree_dirs,
    prune_worktree_dirs_for_space, touch_last_used, touch_last_used_at,
};
use crate::workspace::types::unix_now;

/// A copy's name never begins with `.` or `_`, which Go tools skip as hidden (#391).
#[test]
fn a_copy_is_never_named_as_a_hidden_directory() {
    assert_eq!(sanitize_identifier(".tmpZAcUGt"), "dot-tmpZAcUGt");
    assert_eq!(sanitize_identifier("_scratch"), "under-scratch");
    assert_eq!(sanitize_identifier("prod.codes"), "prod.codes");
    assert_eq!(sanitize_identifier("my repo"), "my_repo");
    assert_eq!(sanitize_identifier(""), "workspace");
}

/// Low on space, idle worktree copies go oldest first until enough is free, however young;
/// one used within the hour and the main copy stay (#386).
#[tokio::test]
async fn worktree_copies_are_pruned_oldest_first_when_space_runs_low() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path();
    let manager = WorkspaceManager::new();
    let aged = |name: &str, hours: u64| {
        let dir = storage.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        touch_last_used(&dir);
        std::fs::File::options()
            .write(true)
            .open(dir.join(LAST_USED_MARKER))
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(hours * 3600))
            .unwrap();
        dir
    };
    let oldest = aged("repo--wt-00000001", 44);
    let older = aged("repo--wt-00000002", 30);
    let old = aged("repo--wt-00000003", 25);
    let in_use = aged("repo--wt-00000004", 0);
    let main = aged("repo", 100);
    // Every copy removed frees ten points: 5% free with four copies, 25% with two left.
    let copies = |root: &Path| {
        std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("--wt-"))
            .count() as f64
    };
    let free = |root: &Path| Some(0.05 + 0.10 * (4.0 - copies(root)));

    let removed = prune_worktree_dirs_for_space(storage, 0.20, &manager, free).await;
    assert_eq!(removed, vec![oldest.clone(), older.clone()]);
    assert!(old.exists() && in_use.exists() && main.exists());

    // With enough free, nothing goes.
    let plenty = |_: &Path| Some(0.50);
    assert!(
        prune_worktree_dirs_for_space(storage, 0.20, &manager, plenty)
            .await
            .is_empty()
    );
    // Out of candidates, the one in use still stays.
    let full = |_: &Path| Some(0.0);
    let rest = prune_worktree_dirs_for_space(storage, 0.20, &manager, full).await;
    assert_eq!(rest, vec![old.clone()]);
    assert!(in_use.exists() && main.exists());
    assert!(free_share(storage).is_some_and(|share| (0.0..=1.0).contains(&share)));
}

#[tokio::test]
async fn test_prune_stale_worktree_dirs() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path();
    let manager = WorkspaceManager::new();
    let old = storage.join("repo--wt-deadbeef");
    let recent = storage.join("repo--wt-cafebabe");
    let main = storage.join("repo");
    for d in [&old, &recent, &main] {
        std::fs::create_dir_all(d).unwrap();
        touch_last_used(d);
    }
    let long_ago = SystemTime::now() - Duration::from_secs(30 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(old.join(LAST_USED_MARKER))
        .unwrap()
        .set_modified(long_ago)
        .unwrap();
    // An old main-repository directory is never pruned, only worktree copies.
    std::fs::File::options()
        .write(true)
        .open(main.join(LAST_USED_MARKER))
        .unwrap()
        .set_modified(long_ago)
        .unwrap();

    let removed =
        prune_stale_worktree_dirs(storage, Duration::from_secs(7 * 86_400), &manager).await;
    assert_eq!(removed, vec![old.clone()]);
    assert!(!old.exists());
    assert!(recent.exists());
    assert!(main.exists());
}

#[tokio::test]
async fn test_prune_stale_main_workspace_dirs() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path();
    let manager = WorkspaceManager::new();

    let old_main = storage.join("repo-old");
    let recent_main = storage.join("repo-recent");
    let hidden = storage.join(".prod-code-shadow");
    let lost_found = storage.join("lost+found");
    let wt = storage.join("repo-old--wt-12345678");

    for d in [&old_main, &recent_main, &hidden, &lost_found, &wt] {
        std::fs::create_dir_all(d).unwrap();
        touch_last_used(d);
    }

    let now = SystemTime::now();
    let two_days_ago = now - Duration::from_secs(2 * 86_400);
    let ten_minutes_ago = now - Duration::from_secs(600);

    // old_main is 2 days old
    touch_last_used_at(&old_main, unix_now().saturating_sub(2 * 86_400));
    let _ = std::fs::File::options()
        .write(true)
        .open(old_main.join(LAST_USED_MARKER))
        .unwrap()
        .set_modified(two_days_ago);

    // recent_main is 10 minutes old
    touch_last_used_at(&recent_main, unix_now().saturating_sub(600));
    let _ = std::fs::File::options()
        .write(true)
        .open(recent_main.join(LAST_USED_MARKER))
        .unwrap()
        .set_modified(ten_minutes_ago);

    // hidden and lost+found are also old
    let _ = std::fs::File::options()
        .write(true)
        .open(hidden.join(LAST_USED_MARKER))
        .unwrap()
        .set_modified(two_days_ago);
    let _ = std::fs::File::options()
        .write(true)
        .open(lost_found.join(LAST_USED_MARKER))
        .unwrap()
        .set_modified(two_days_ago);

    // wt is also old, but prune_stale_main_workspace_dirs ignores worktrees
    let _ = std::fs::File::options()
        .write(true)
        .open(wt.join(LAST_USED_MARKER))
        .unwrap()
        .set_modified(two_days_ago);

    // Pruning with 1-day (86400s) timeout
    let removed = prune_stale_main_workspace_dirs(
        storage,
        Duration::from_secs(86_400),
        Duration::from_secs(3600),
        &manager,
    )
    .await;

    assert_eq!(removed, vec![old_main.clone()]);
    assert!(!old_main.exists());
    assert!(recent_main.exists());
    assert!(hidden.exists());
    assert!(lost_found.exists());
    assert!(wt.exists()); // not pruned by main workspace pruner
}

#[tokio::test]
async fn test_prune_stale_main_workspace_protected_by_active_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path();
    let manager = WorkspaceManager::new();

    let main = storage.join("active-project");
    let active_wt = storage.join("active-project--wt-abcdef12");

    std::fs::create_dir_all(&main).unwrap();
    std::fs::create_dir_all(&active_wt).unwrap();
    touch_last_used(&main);
    touch_last_used(&active_wt);

    let now = SystemTime::now();
    let two_days_ago = now - Duration::from_secs(2 * 86_400);

    // Main is old (2 days ago)
    let _ = std::fs::File::options()
        .write(true)
        .open(main.join(LAST_USED_MARKER))
        .unwrap()
        .set_modified(two_days_ago);

    // Active worktree is fresh (just touched)
    // Main should be protected because active_wt is idle < 3600s
    let removed = prune_stale_main_workspace_dirs(
        storage,
        Duration::from_secs(86_400),
        Duration::from_secs(3600),
        &manager,
    )
    .await;
    assert!(removed.is_empty());
    assert!(main.exists());
    assert!(active_wt.exists());

    // Now age the worktree to 2 hours ago
    let two_hours_ago = now - Duration::from_secs(2 * 3600);
    let _ = std::fs::File::options()
        .write(true)
        .open(active_wt.join(LAST_USED_MARKER))
        .unwrap()
        .set_modified(two_hours_ago);

    // First worktree pruner cleans up stale worktree
    let wt_removed = prune_stale_worktree_dirs(storage, Duration::from_secs(3600), &manager).await;
    assert_eq!(wt_removed, vec![active_wt.clone()]);
    assert!(!active_wt.exists());

    // Now main workspace has no active worktrees, and can be pruned
    let main_removed = prune_stale_main_workspace_dirs(
        storage,
        Duration::from_secs(86_400),
        Duration::from_secs(3600),
        &manager,
    )
    .await;
    assert_eq!(main_removed, vec![main.clone()]);
    assert!(!main.exists());
}
