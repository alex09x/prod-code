/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::fixtures::host;
use crate::workspace::manager::WorkspaceManager;
use crate::workspace::paths::worktree_suffix;

#[tokio::test]
async fn test_worktree_overlay_preserved_during_unload_while_active_lease_exists() {
    let temp_dir = tempfile::tempdir().unwrap();
    let storage = temp_dir.path();
    let base_dir = storage.join("sample-repo");
    std::fs::create_dir_all(base_dir.join("src")).unwrap();
    std::fs::write(
        base_dir.join("Cargo.toml"),
        "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        base_dir.join("src/lib.rs"),
        "pub fn base_func() -> u32 { 42 }\n",
    )
    .unwrap();

    let wt_dir = storage.join(format!("sample-repo{}", worktree_suffix("client-wt-path")));
    std::fs::create_dir_all(wt_dir.join("src")).unwrap();
    std::fs::write(
        wt_dir.join("Cargo.toml"),
        "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        wt_dir.join("src/lib.rs"),
        "pub fn base_func() -> u32 { 100 }\n",
    )
    .unwrap();

    let manager = Arc::new(WorkspaceManager::new());
    let _base_lease = manager.get_or_load(&base_dir, "rust").await.unwrap();
    let wt_lease = manager.get_or_load(&wt_dir, "rust").await.unwrap();
    let base_ws = Arc::clone(wt_lease.base_workspace.as_ref().unwrap());
    let base_eng = Arc::clone(base_ws.rust_engine.as_ref().unwrap());

    // Overlay is attached and base worktree count is 1
    assert!(base_eng.lock().await.has_worktree(&wt_dir));
    assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 1);
    assert_eq!(
        wt_lease.workspace().active_sessions.load(Ordering::Relaxed),
        1
    );

    // Unload the worktree workspace while lease is still held (e.g. manifest change during session)
    let unloaded = manager.unload_under(&wt_dir).await;
    assert_eq!(unloaded, 1);
    assert!(!manager.is_loaded(&wt_dir).await);

    // CRITICAL: Overlay MUST remain attached and base pin preserved for extant lease
    assert!(
        base_eng.lock().await.has_worktree(&wt_dir),
        "overlay must remain attached while lease is active even after unload_under"
    );
    assert_eq!(
        base_ws.attached_worktrees.load(Ordering::Relaxed),
        1,
        "base attached_worktrees must not decrement while lease is active"
    );

    // Dropping the active lease triggers final detachment
    drop(wt_lease);

    // Wait briefly for the detachment background task if spawned
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !base_eng.lock().await.has_worktree(&wt_dir)
                && base_ws.attached_worktrees.load(Ordering::Relaxed) == 0
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("overlay detachment after final lease drop");

    assert!(!base_eng.lock().await.has_worktree(&wt_dir));
    assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn test_repeated_validation_views_do_not_leak_validation_engine_attachment_refcount() {
    let temp_dir = tempfile::tempdir().unwrap();
    let storage = temp_dir.path();
    let base_dir = storage.join("sample-repo");
    std::fs::create_dir_all(base_dir.join("src")).unwrap();
    std::fs::write(
        base_dir.join("Cargo.toml"),
        "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        base_dir.join("src/lib.rs"),
        "pub fn base_func() -> u32 { 42 }\n",
    )
    .unwrap();

    let wt_dir = storage.join(format!("sample-repo{}", worktree_suffix("client-wt-path")));
    std::fs::create_dir_all(wt_dir.join("src")).unwrap();
    std::fs::write(
        wt_dir.join("Cargo.toml"),
        "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        wt_dir.join("src/lib.rs"),
        "pub fn base_func() -> u32 { 100 }\n",
    )
    .unwrap();

    let manager = Arc::new(WorkspaceManager::new());
    let _base_lease = manager.get_or_load(&base_dir, "rust").await.unwrap();
    let wt_lease = manager.get_or_load(&wt_dir, "rust").await.unwrap();
    let wt_ws = wt_lease.workspace();
    let base_ws = Arc::clone(wt_ws.base_workspace.as_ref().unwrap());

    let admission = Arc::new(crate::admission::Admission::unbounded());

    // Repeatedly request validation views on the worktree
    let mut val_views = Vec::new();
    for _ in 0..5 {
        let val_ws = wt_ws.validation_view(&admission).await.unwrap();
        val_views.push(val_ws);
    }

    // Get the validation engine from the base workspace
    let val_eng_arc = base_ws
        .validation
        .get()
        .and_then(|opt| opt.as_ref())
        .expect("validation engine must be initialized");

    {
        let val_eng = val_eng_arc.lock().await;
        assert!(val_eng.has_worktree(&wt_dir));
        assert_eq!(
            val_eng.worktree_attachment_count(&wt_dir),
            1,
            "validation engine attachment count must be 1 regardless of repeated validation views"
        );
    }

    // Drop transient validation views
    drop(val_views);

    // Overlay remains attached while worktree workspace is alive
    assert!(val_eng_arc.lock().await.has_worktree(&wt_dir));

    // Unload and drop lease to trigger detachment
    drop(wt_lease);
    let unloaded = manager.unload_under(&wt_dir).await;
    assert_eq!(unloaded, 1);

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !val_eng_arc.lock().await.has_worktree(&wt_dir) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("validation engine overlay detachment after worktree unload");

    assert!(
        !val_eng_arc.lock().await.has_worktree(&wt_dir),
        "validation engine overlay must be detached after worktree is unloaded and retired"
    );
    assert_eq!(
        val_eng_arc.lock().await.worktree_attachment_count(&wt_dir),
        0
    );
}

#[tokio::test]
async fn test_validation_fallback_does_not_leak_main_engine_attachment() {
    let temp_dir = tempfile::tempdir().unwrap();
    let storage = temp_dir.path();
    let base_dir = storage.join("sample-repo");
    std::fs::create_dir_all(base_dir.join("src")).unwrap();
    std::fs::write(
        base_dir.join("Cargo.toml"),
        "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        base_dir.join("src/lib.rs"),
        "pub fn base_func() -> u32 { 42 }\n",
    )
    .unwrap();

    let wt_dir = storage.join(format!("sample-repo{}", worktree_suffix("client-wt-path")));
    std::fs::create_dir_all(wt_dir.join("src")).unwrap();
    std::fs::write(
        wt_dir.join("Cargo.toml"),
        "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        wt_dir.join("src/lib.rs"),
        "pub fn base_func() -> u32 { 100 }\n",
    )
    .unwrap();

    let manager = Arc::new(WorkspaceManager::new());
    let _base_lease = manager.get_or_load(&base_dir, "rust").await.unwrap();
    let wt_lease = manager.get_or_load(&wt_dir, "rust").await.unwrap();
    let wt_ws = wt_lease.workspace();
    let main_eng_arc = Arc::clone(wt_ws.rust_engine.as_ref().unwrap());

    // Base worktree attachment count is 1 initially
    {
        let eng = main_eng_arc.lock().await;
        assert!(eng.has_worktree(&wt_dir));
        assert_eq!(eng.worktree_attachment_count(&wt_dir), 1);
    }

    // Host has 95 of 100 GiB used, so admission refuses a second validation engine
    let refused_admission = Arc::new(crate::admission::Admission::with_probe(
        crate::admission::scripted_probe(vec![host(95, 100)]),
        2048,
        Duration::ZERO,
    ));

    // Call validation_view when admission has no capacity: falls back to main engine
    let val_ws = wt_ws.validation_view(&refused_admission).await.unwrap();
    assert!(Arc::ptr_eq(
        val_ws.rust_engine.as_ref().unwrap(),
        &main_eng_arc
    ));

    // CRITICAL: The main engine attachment count must still be 1 (NOT 2)
    {
        let eng = main_eng_arc.lock().await;
        assert_eq!(
            eng.worktree_attachment_count(&wt_dir),
            1,
            "validation fallback must not acquire a second attachment on the main engine"
        );
    }

    drop(val_ws);
    drop(wt_lease);

    // Unload the worktree
    let unloaded = manager.unload_under(&wt_dir).await;
    assert_eq!(unloaded, 1);

    // Detach background task
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !main_eng_arc.lock().await.has_worktree(&wt_dir) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("main engine overlay detachment after unload");

    // The overlay must be completely gone and refcount 0
    {
        let eng = main_eng_arc.lock().await;
        assert!(!eng.has_worktree(&wt_dir));
        assert_eq!(eng.worktree_attachment_count(&wt_dir), 0);
    }
}
