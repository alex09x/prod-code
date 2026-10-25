/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use tokio::sync::Mutex;

fn checkout(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname='coalesced_fixture'\nversion='0.1.0'\nedition='2021'\n[lib]\npath='lib.rs'\n").unwrap();
    std::fs::write(
        root.join("prod-code.toml"),
        "[rust]\nbuild_scripts=false\nproc_macro_srv='disabled'\n",
    )
    .unwrap();
    std::fs::write(root.join("lib.rs"), "pub fn sample() {}\n").unwrap();
}

#[tokio::test]
async fn failed_mirror_attachment_rolls_back_only_the_new_reference_and_can_retry() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("attach-base");
    let copy = temp.path().join("attach-base--wt-copy");
    checkout(&base);
    checkout(&copy);
    let primary = Arc::new(Mutex::new(
        prod_code_engine_rust::RustEngine::load(&base).unwrap(),
    ));
    let mirror = Arc::new(Mutex::new(
        prod_code_engine_rust::RustEngine::load(&base).unwrap(),
    ));
    primary.lock().await.attach_worktree(&copy).unwrap();
    // The primary has a cached workspace, whereas the mirror must discover its manifest.
    std::fs::remove_file(copy.join("Cargo.toml")).unwrap();
    let expected = format!(
        "Failed to attach worktree: {:#}",
        mirror.lock().await.attach_worktree(&copy).unwrap_err()
    );
    let base_ws = Arc::new(SharedWorkspace::new(
        base.clone(),
        "rust".into(),
        Some(Arc::clone(&primary)),
        None,
        None,
        None,
    ));
    base_ws
        .rust_engines
        .lock()
        .unwrap()
        .push(Arc::clone(&mirror));
    let manager = Arc::new(WorkspaceManager::new());
    manager.insert_ready_for_test(Arc::clone(&base_ws)).await;
    let stale = Arc::new(SharedWorkspace::new(
        copy.clone(),
        "other".into(),
        None,
        None,
        None,
        None,
    ));
    manager.insert_ready_for_test(Arc::clone(&stale)).await;
    let error = manager.get_or_load(&copy, "rust").await.err().unwrap();
    assert_eq!(error.to_string(), expected);
    assert!(stale.unloaded.load(Ordering::SeqCst));
    assert!(
        !manager
            .workspaces
            .read()
            .await
            .contains_key(&WorkspaceKey(copy.clone()))
    );
    assert_eq!(primary.lock().await.worktree_attachment_count(&copy), 1);
    assert_eq!(mirror.lock().await.worktree_attachment_count(&copy), 0);
    assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 0);
    assert_eq!(base_ws.active_sessions.load(Ordering::Relaxed), 0);

    checkout(&copy);
    let lease = manager.get_or_load(&copy, "rust").await.unwrap();
    assert!(Arc::ptr_eq(
        lease.base_workspace.as_ref().unwrap(),
        &base_ws
    ));
    assert!(Arc::ptr_eq(lease.rust_engine.as_ref().unwrap(), &primary));
    assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 1);
    assert_eq!(primary.lock().await.worktree_attachment_count(&copy), 2);
    assert_eq!(mirror.lock().await.worktree_attachment_count(&copy), 1);
    let second = manager.get_or_load(&copy, "rust").await.unwrap();
    assert!(Arc::ptr_eq(lease.workspace(), second.workspace()));
    assert_eq!(lease.active_sessions.load(Ordering::Relaxed), 2);
    // Retiring the workspace releases its one attachment on every mirrored engine.
    drop(second);
    drop(lease);
    assert_eq!(manager.unload_under(&copy).await, 1);
    assert_eq!(primary.lock().await.worktree_attachment_count(&copy), 1);
    assert_eq!(mirror.lock().await.worktree_attachment_count(&copy), 0);
    assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 0);
    assert!(primary.lock().await.detach_worktree(&copy));
}

#[tokio::test]
async fn replacing_an_engine_publishes_one_new_workspace_and_releases_the_old_one() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("replace-workspace");
    let manager = Arc::new(WorkspaceManager::new());
    let old = Arc::new(SharedWorkspace::new(
        root.clone(),
        "old".into(),
        None,
        None,
        None,
        None,
    ));
    manager.insert_ready_for_test(Arc::clone(&old)).await;
    let lease = manager
        .get_or_load_direct(&root, "unavailable-fixture")
        .await
        .unwrap();
    assert_eq!(lease.engine, "unavailable-fixture");
    assert_eq!(lease.root, root);
    assert!(old.unloaded.load(Ordering::SeqCst));
    assert!(!Arc::ptr_eq(&old, lease.workspace()));
    assert!(Arc::ptr_eq(
        lease.workspace(),
        manager
            .get_or_load_direct(&root, "unavailable-fixture")
            .await
            .unwrap()
            .workspace()
    ));
    assert_eq!(manager.loaded_count().await, 1);
}

#[tokio::test]
async fn worktree_without_a_base_rust_engine_loads_its_own_engine() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("fallback-base");
    let copy = temp.path().join("fallback-copy");
    checkout(&base);
    checkout(&copy);
    let manager = Arc::new(WorkspaceManager::new());
    let base_ws = Arc::new(SharedWorkspace::new(
        base.clone(),
        "rust".into(),
        None,
        None,
        None,
        None,
    ));
    manager.insert_ready_for_test(Arc::clone(&base_ws)).await;
    let lease = manager.get_or_load_worktree(&copy, &base).await.unwrap();
    assert_eq!(lease.root, copy);
    assert_eq!(lease.engine, "rust");
    assert!(lease.base_workspace.is_none());
    assert!(lease.rust_engine.is_some());
    assert_eq!(
        lease
            .rust_engine
            .as_ref()
            .unwrap()
            .lock()
            .await
            .document_symbols(&copy.join("lib.rs"))
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect::<Vec<_>>(),
        ["sample"]
    );
    assert_eq!(base_ws.active_sessions.load(Ordering::Relaxed), 0);
    assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 0);
    assert!(Arc::ptr_eq(
        lease.workspace(),
        &manager.get_loaded(&copy).await.unwrap()
    ));
    drop(lease);
    assert_eq!(manager.unload_under(&copy).await, 1);
}
