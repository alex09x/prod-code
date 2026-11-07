/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Loading a workspace ahead of any request must never replace what is there (#1976): a root
//! holds one workspace, whatever its engine, and a load of another engine replaces it.

use super::*;
use crate::warmup::Ahead;

fn workspace(root: &Path, engine: &str) -> Arc<SharedWorkspace> {
    Arc::new(SharedWorkspace::new(
        root.to_path_buf(),
        engine.into(),
        None,
        None,
        None,
        None,
    ))
}

#[tokio::test]
async fn a_workspace_of_another_engine_is_left_as_it_is() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("mixed");
    let manager = Arc::new(WorkspaceManager::new());
    let live = workspace(&root, "old");
    manager.insert_ready_for_test(Arc::clone(&live)).await;

    let outcome = manager
        .load_ahead(&root, "unavailable-fixture")
        .await
        .unwrap();

    assert_eq!(outcome, Ahead::Present);
    assert!(!live.unloaded.load(Ordering::SeqCst), "not unloaded");
    assert_eq!(manager.loaded_count().await, 1);
    let guard = manager.workspaces.read().await;
    let Some(LoadState::Ready(kept)) = guard.get(&WorkspaceKey(root)) else {
        panic!("the workspace is still there");
    };
    assert!(
        Arc::ptr_eq(kept, &live),
        "the same workspace, not a new one"
    );
}

#[tokio::test]
async fn a_root_that_is_loading_is_left_to_its_leader() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("loading");
    let manager = Arc::new(WorkspaceManager::new());
    let (tx, _rx) = broadcast::channel(1);
    manager
        .workspaces
        .write()
        .await
        .insert(WorkspaceKey(root.clone()), LoadState::Loading(tx));

    let outcome = manager
        .load_ahead(&root, "unavailable-fixture")
        .await
        .unwrap();

    assert_eq!(outcome, Ahead::Present);
    assert_eq!(
        manager.loaded_count().await,
        0,
        "nothing of ours was loaded"
    );
}

#[tokio::test]
async fn an_empty_root_is_loaded_once_and_then_is_there() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("empty");
    let manager = Arc::new(WorkspaceManager::new());

    assert_eq!(
        manager
            .load_ahead(&root, "unavailable-fixture")
            .await
            .unwrap(),
        Ahead::Loaded
    );
    assert_eq!(manager.loaded_count().await, 1);
    assert_eq!(
        manager
            .load_ahead(&root, "unavailable-fixture")
            .await
            .unwrap(),
        Ahead::Present,
        "a second load finds it"
    );
    assert_eq!(manager.loaded_count().await, 1);
}

#[tokio::test]
async fn a_worktree_is_left_alone_so_that_its_base_is_not_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("ahead-base");
    let copy = temp.path().join("ahead-base--wt-copy");
    super::worktrees::checkout(&base);
    super::worktrees::checkout(&copy);
    let manager = Arc::new(WorkspaceManager::new());
    let held = workspace(&base, "python");
    manager.insert_ready_for_test(Arc::clone(&held)).await;

    let outcome = manager.load_ahead(&copy, "rust").await.unwrap();

    assert_eq!(outcome, Ahead::Worktree);
    assert!(!held.unloaded.load(Ordering::SeqCst), "the base is kept");
    assert_eq!(manager.loaded_count().await, 1);
}
