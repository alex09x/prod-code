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
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::workspace::manager::WorkspaceManager;
use crate::workspace::shared::SharedWorkspace;
use crate::workspace::types::unix_now;

#[tokio::test]
async fn test_leader_follower_coalescing() {
    let manager = Arc::new(WorkspaceManager::with_admission(Arc::new(
        crate::admission::Admission::unbounded(),
    )));
    let root = PathBuf::from("/test/workspace");

    // Concurrent requests for the same workspace
    let m1 = Arc::clone(&manager);
    let r1 = root.clone();
    let handle1 = tokio::spawn(async move { m1.get_or_load(&r1, "rust").await.unwrap() });

    let m2 = Arc::clone(&manager);
    let r2 = root.clone();
    let handle2 = tokio::spawn(async move { m2.get_or_load(&r2, "rust").await.unwrap() });

    let (ws1, ws2) = tokio::join!(handle1, handle2);
    let ws1 = ws1.unwrap();
    let ws2 = ws2.unwrap();

    // Both sessions share the exact same Arc instance in memory!
    assert!(Arc::ptr_eq(ws1.workspace(), ws2.workspace()));
    assert_eq!(manager.loaded_count().await, 1);
    assert_eq!(ws1.active_sessions.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn test_single_owner_detection() {
    let manager = Arc::new(WorkspaceManager::with_admission(Arc::new(
        crate::admission::Admission::unbounded(),
    )));
    let root = PathBuf::from("/test/repo");
    let wt1 = PathBuf::from("/test/repo/worktree-1");
    let wt2 = PathBuf::from("/test/repo/worktree-2");

    let view1 = manager
        .register_session_view(
            1,
            wt1.clone(),
            manager.get_or_load(&root, "rust").await.unwrap(),
        )
        .await;
    assert!(view1.is_single_owner(), "First agent on wt1 is sole owner");

    let view2 = manager
        .register_session_view(
            2,
            wt2.clone(),
            manager.get_or_load(&root, "rust").await.unwrap(),
        )
        .await;
    assert!(view2.is_single_owner(), "First agent on wt2 is sole owner");

    // Second session attaches to wt1
    let view3 = manager
        .register_session_view(
            3,
            wt1.clone(),
            manager.get_or_load(&root, "rust").await.unwrap(),
        )
        .await;
    assert!(
        !view3.is_single_owner(),
        "Second agent on wt1 is NOT sole owner"
    );
    assert!(
        !view1.is_single_owner(),
        "First agent on wt1 direct-edit exclusivity was revoked when second session joined"
    );

    // Cleanup
    manager.unregister_session_view(view1).await;
    manager.unregister_session_view(view2).await;
    manager.unregister_session_view(view3).await;
}

#[tokio::test]
async fn test_single_owner_direct_edit_fast_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let cargo_toml = root.join("Cargo.toml");
    std::fs::write(
        &cargo_toml,
        r#"[package]
name = "fast_path_fixture"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
    )
    .unwrap();
    let lib_path = src_dir.join("lib.rs");
    std::fs::write(&lib_path, "pub const BASE_VAL: u32 = 100;\n").unwrap();

    let engine = prod_code_engine_rust::RustEngine::load(&root).expect("load engine");
    let engine_arc = Arc::new(Mutex::new(engine));
    let ws = Arc::new(SharedWorkspace::new(
        root.clone(),
        "rust".to_string(),
        Some(Arc::clone(&engine_arc)),
        None,
        None,
        None,
    ));

    let manager = Arc::new(WorkspaceManager::with_admission(Arc::new(
        crate::admission::Admission::unbounded(),
    )));
    manager.insert_ready_for_test(Arc::clone(&ws)).await;

    let lease1 = manager.get_or_load(&root, "rust").await.unwrap();
    let view1 = manager
        .register_session_view(101, root.clone(), lease1)
        .await;
    assert!(
        view1.is_single_owner(),
        "Dedicated worktree is single owner"
    );

    let direct_text = "pub const BASE_VAL: u32 = 100;\npub fn direct_added() {}\n".to_string();
    // Fast path: direct edit modifies base Salsa input without session overlays
    {
        let mut eng = engine_arc.lock().await;
        assert!(!eng.has_session_overlays());
        eng.apply_file_change(&lib_path, direct_text.clone())
            .unwrap();
        assert!(
            !eng.has_session_overlays(),
            "Direct edits must not create session overlays"
        );
        assert_eq!(eng.session_overlay_count(view1.session_id), 0);

        let syms = eng.document_symbols(&lib_path).unwrap();
        assert!(syms.iter().any(|s| s.name == "direct_added"));
    }

    // Track open file in view1
    view1
        .direct_edit_open_files
        .lock()
        .unwrap()
        .insert(lib_path.clone(), direct_text);

    // When a second session joins the same worktree, direct-edit exclusivity must be revoked
    // and view1's direct edits migrated into view1's session overlay in the engine!
    let lease2 = manager.get_or_load(&root, "rust").await.unwrap();
    let view2 = manager
        .register_session_view(102, root.clone(), lease2)
        .await;
    assert!(
        !view1.is_single_owner(),
        "view1 exclusivity must be revoked when view2 joins"
    );
    assert!(
        !view2.is_single_owner(),
        "view2 must not have single-owner exclusivity"
    );

    {
        let mut eng = engine_arc.lock().await;
        // The engine now has session overlays for view1
        assert!(eng.has_session_overlays());
        assert_eq!(eng.session_overlay_count(view1.session_id), 1);
        assert_eq!(eng.session_overlay_count(view2.session_id), 0);

        // Base Salsa DB was restored from disk!
        eng.activate_session(view2.session_id).unwrap();
        let view2_syms = eng.document_symbols(&lib_path).unwrap();
        assert!(
            !view2_syms.iter().any(|s| s.name == "direct_added"),
            "view2 must NOT observe view1's unsaved direct edits in base Salsa DB!"
        );

        // view1's unsaved edits are preserved in its session overlay!
        eng.activate_session(view1.session_id).unwrap();
        let view1_syms = eng.document_symbols(&lib_path).unwrap();
        assert!(
            view1_syms.iter().any(|s| s.name == "direct_added"),
            "view1 must observe its unsaved edits in its session overlay"
        );
    }

    // Unregister both sessions
    manager.unregister_session_view(view1).await;
    manager.unregister_session_view(view2).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    {
        let eng = engine_arc.lock().await;
        assert!(!eng.has_session_overlays());
        let syms = eng.document_symbols(&lib_path).unwrap();
        assert!(
            !syms.iter().any(|s| s.name == "direct_added"),
            "Disk state must be restored after both sessions retire"
        );
    }
}

#[tokio::test]
async fn test_evict_idle_drops_only_idle_unused_workspaces() {
    let manager = WorkspaceManager::new();
    let idle = Arc::new(SharedWorkspace::new(
        PathBuf::from("/srv/ws/idle"),
        "rust".to_string(),
        None,
        None,
        None,
        None,
    ));
    idle.last_used.store(unix_now() - 7200, Ordering::Relaxed);
    let busy = Arc::new(SharedWorkspace::new(
        PathBuf::from("/srv/ws/busy"),
        "rust".to_string(),
        None,
        None,
        None,
        None,
    ));
    busy.last_used.store(unix_now() - 7200, Ordering::Relaxed);
    busy.active_sessions.store(1, Ordering::Relaxed);
    let fresh = Arc::new(SharedWorkspace::new(
        PathBuf::from("/srv/ws/fresh"),
        "rust".to_string(),
        None,
        None,
        None,
        None,
    ));
    manager.insert_ready_for_test(idle).await;
    manager.insert_ready_for_test(busy).await;
    manager.insert_ready_for_test(fresh).await;

    let evicted = manager.evict_idle(Duration::from_secs(3600)).await;
    assert_eq!(evicted, vec![PathBuf::from("/srv/ws/idle")]);
    assert_eq!(manager.loaded_count().await, 2);
    assert!(manager.is_loaded(Path::new("/srv/ws/busy")).await);
    assert!(!manager.is_loaded(Path::new("/srv/ws/idle")).await);
}
