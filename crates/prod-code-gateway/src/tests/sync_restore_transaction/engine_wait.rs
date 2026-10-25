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
use crate::sync::test_hooks::{Installation, Point};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_restore_transaction_review_engine_wait_allows_following_sync_and_probe() {
    let storage = tempfile::tempdir().unwrap();
    let root = workspace::resolve_server_workspace(
        storage.path(),
        "/tmp/engine-wait",
        Some("engine-wait"),
    );
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"engine_wait\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub const VALUE: u32 = 1;\n").unwrap();
    workspace::touch_last_used(&root);
    let engine = Arc::new(tokio::sync::Mutex::new(
        prod_code_engine_rust::RustEngine::load(&root).unwrap(),
    ));
    let manager = Arc::new(WorkspaceManager::new());
    manager
        .insert_ready_for_test(Arc::new(workspace::SharedWorkspace::new(
            root.clone(),
            "rust".into(),
            Some(engine.clone()),
            None,
            None,
            None,
        )))
        .await;
    let started = Instant::now();
    let before = Arc::new(snapshot_tree(&root));
    std::fs::write(root.join("src/lib.rs"), "pub const VALUE: u32 = 99;\n").unwrap();
    let held_engine = engine.clone().lock_owned().await;
    let hook = Installation::new(&root, Point::RestoreDisk);
    let mut restore = tokio::spawn({
        let manager = manager.clone();
        let root = root.clone();
        async move { restore_after_lost_client(&manager, &root, before, started).await }
    });
    hook.entered().await;
    hook.release();
    let progress = tokio::time::timeout(Duration::from_secs(8), async {
        let response = apply_sync(
            storage.path(),
            &manager,
            SyncRequest {
                client_workspace_root: "/tmp/engine-wait".into(),
                base_workspace_name: Some("engine-wait".into()),
                files: vec![write_delta("src/lib.rs", "pub const VALUE: u32 = 2;\n")],
                clean_others: false,
            },
        )
        .await;
        let probe = crate::sync::apply_sync_probe(
            storage.path(),
            &manager,
            prod_code_protocol::SyncProbeRequest {
                client_workspace_root: "/tmp/engine-wait".into(),
                base_workspace_name: Some("engine-wait".into()),
                seed_from: None,
                files: stamp_tree(&root)
                    .into_iter()
                    .map(
                        |(relative_path, (size, hash))| prod_code_protocol::FileStamp {
                            relative_path,
                            size,
                            hash,
                        },
                    )
                    .collect(),
            },
        )
        .await;
        (response, probe)
    })
    .await;
    let bounded_restore = tokio::time::timeout(Duration::from_secs(2), &mut restore).await;
    let restore_finished_while_engine_busy = bounded_restore.is_ok();
    drop(held_engine);
    let restored = match bounded_restore {
        Ok(result) => result.unwrap(),
        Err(_) => tokio::time::timeout(Duration::from_secs(10), restore)
            .await
            .unwrap()
            .unwrap(),
    };
    assert_eq!(restored, 1);
    let (response, probe) =
        progress.expect("following sync/probe blocked behind restore's engine wait");
    assert_eq!(response.files_updated, 1);
    assert!(probe.missing.is_empty());
    assert!(
        restore_finished_while_engine_busy,
        "restore must finish within its engine update budget"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        "pub const VALUE: u32 = 2;\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_restore_transaction_deferred_restore_reads_current_disk_and_invalidates_manifest() {
    let storage = tempfile::tempdir().unwrap();
    let root =
        workspace::resolve_server_workspace(storage.path(), "/tmp/deferred", Some("deferred"));
    std::fs::create_dir_all(root.join("src")).unwrap();
    let manifest = "[package]\nname = \"deferred\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
    std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn original() {}\n").unwrap();
    workspace::touch_last_used(&root);
    let engine = Arc::new(tokio::sync::Mutex::new(
        prod_code_engine_rust::RustEngine::load(&root).unwrap(),
    ));
    let manager = WorkspaceManager::new();
    manager
        .insert_ready_for_test(Arc::new(workspace::SharedWorkspace::new(
            root.clone(),
            "rust".into(),
            Some(engine.clone()),
            None,
            None,
            None,
        )))
        .await;
    let started = Instant::now();
    let before = Arc::new(snapshot_tree(&root));
    std::fs::write(root.join("src/lib.rs"), "pub fn command_dirty() {}\n").unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        format!("{manifest}\n# command changed manifest\n"),
    )
    .unwrap();
    let held_engine = engine.clone().lock_owned().await;
    let restored = tokio::time::timeout(
        Duration::from_secs(7),
        restore_after_lost_client(&manager, &root, before, started),
    )
    .await;
    // Release the engine even on a failure before asserting, so no deferred worker is stranded.
    if restored.is_err() {
        drop(held_engine);
        panic!("restore exceeded its bounded engine update wait");
    }
    assert_eq!(restored.unwrap(), 2);
    assert!(
        manager.get_loaded(&root).await.is_none(),
        "restored manifest must invalidate the loaded workspace while its engine is busy"
    );
    let newer = "pub fn newer_sync() {}\n";
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        apply_sync(
            storage.path(),
            &manager,
            SyncRequest {
                client_workspace_root: "/tmp/deferred".into(),
                base_workspace_name: Some("deferred".into()),
                files: vec![write_delta("src/lib.rs", newer)],
                clean_others: false,
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(response.files_updated, 1);
    drop(held_engine);
    // The deferred restore queued its engine lock before returning. This FIFO acquisition
    // fences the real update, with no later sync engine update to hide stale restore bytes.
    let engine = tokio::time::timeout(Duration::from_secs(10), engine.lock())
        .await
        .unwrap();
    let symbols = engine.document_symbols(&root.join("src/lib.rs")).unwrap();
    assert!(
        symbols.iter().any(|symbol| symbol.name == "newer_sync"),
        "deferred restore must read newer sync bytes under the engine lock: {symbols:?}"
    );
    assert!(!symbols.iter().any(|symbol| symbol.name == "original"));
    assert_eq!(
        std::fs::read_to_string(root.join("Cargo.toml")).unwrap(),
        manifest
    );
}
