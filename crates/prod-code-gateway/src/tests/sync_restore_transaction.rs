/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::*;
use prod_code_protocol::{FileDelta, SyncRequest, WireMessage, content_hash};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

mod failed_deletes;

fn noop_waker() -> Waker {
    Waker::noop().clone()
}

fn write_delta(relative_path: &str, text: &str) -> FileDelta {
    FileDelta {
        relative_path: relative_path.to_string(),
        content: Some(text.as_bytes().to_vec()),
        is_executable: false,
    }
}

fn delete_delta(relative_path: &str) -> FileDelta {
    FileDelta {
        relative_path: relative_path.to_string(),
        content: None,
        is_executable: false,
    }
}

async fn make_session_view(
    manager: &WorkspaceManager,
    root: &std::path::Path,
    session_id: u64,
) -> (
    workspace::SessionView,
    crate::session::SharedOutputSender,
    rapidfire::mpsc::Receiver<crate::session::SharedOutputFrame>,
) {
    let shared_ws = Arc::new(workspace::SharedWorkspace::new(
        root.to_path_buf(),
        "generic-lsp".to_string(),
        None,
        None,
        None,
        None,
    ));
    let lease = workspace::WorkspaceLease::acquire(shared_ws);
    let view = manager
        .register_session_view(session_id, root.to_path_buf(), lease)
        .await;
    let (raw_tx, raw_rx) = rapidfire::mpsc::bounded(8);
    let out_tx = crate::session::SharedOutputSender::new(raw_tx, Duration::from_secs(1));
    (view, out_tx, raw_rx)
}

#[cfg(unix)]
#[tokio::test]
async fn sync_restore_transaction_session_sync_records_synced_and_protects_restore() {
    use std::os::unix::fs::PermissionsExt;

    let storage = tempfile::tempdir().unwrap();
    let root = workspace::resolve_server_workspace(storage.path(), "/tmp/session-ws", Some("ws"));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("readonly")).unwrap();

    let kept_rel = "src/kept_by_session.rs";
    let deleted_rel = "src/deleted_by_session.rs";
    let failed_rel = "readonly/failed_write.rs";
    let cmd_rel = "src/cmd_only.rs";

    std::fs::write(root.join(kept_rel), "pub fn kept() -> u32 { 1 }\n").unwrap();
    std::fs::write(root.join(deleted_rel), "pub fn gone() {}\n").unwrap();
    std::fs::write(root.join(failed_rel), "pub fn before_cmd() {}\n").unwrap();
    std::fs::write(root.join(cmd_rel), "pub fn cmd() -> u32 { 1 }\n").unwrap();

    let snapshot_started = Instant::now();
    let before = Arc::new(snapshot_tree(&root));
    let manager = workspace::WorkspaceManager::new();
    let (view, out_tx, mut raw_rx) = make_session_view(&manager, &root, 1).await;

    // Verify in-session sync shares the workspace sync lock before mutating disk.
    let held_sync = crate::sync::server_workspace_sync_lock(&root)
        .lock_owned()
        .await;
    let mut locked_sync_fut = Box::pin(crate::session::handle_session_sync(
        SyncRequest {
            client_workspace_root: "/tmp/session-ws".to_string(),
            files: vec![write_delta(kept_rel, "pub fn kept() -> u32 { 2 }\n")],
            clean_others: false,
            base_workspace_name: Some("ws".to_string()),
        },
        &view,
        &out_tx,
    ));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(
        matches!(locked_sync_fut.as_mut().poll(&mut cx), Poll::Pending),
        "session sync must wait on the workspace sync lock"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(kept_rel)).unwrap(),
        "pub fn kept() -> u32 { 1 }\n",
        "session sync must not write to disk while workspace sync lock is held"
    );
    drop(locked_sync_fut);
    drop(held_sync);

    // Command dirties cmd_only.rs and failed_write.rs; readonly dir then blocks the client write.
    std::fs::write(root.join(cmd_rel), "pub fn cmd() -> u32 { 99 }\n").unwrap();
    std::fs::write(root.join(failed_rel), "pub fn cmd_dirty() {}\n").unwrap();
    std::fs::set_permissions(
        root.join("readonly"),
        std::fs::Permissions::from_mode(0o555),
    )
    .unwrap();

    crate::session::handle_session_sync(
        SyncRequest {
            client_workspace_root: "/tmp/session-ws".to_string(),
            files: vec![
                write_delta(kept_rel, "pub fn kept() -> u32 { 2 }\n"),
                delete_delta(deleted_rel),
                write_delta(failed_rel, "pub fn client_unwritten() {}\n"),
            ],
            clean_others: false,
            base_workspace_name: Some("ws".to_string()),
        },
        &view,
        &out_tx,
    )
    .await;

    std::fs::set_permissions(
        root.join("readonly"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let WireMessage::SyncResponse(resp) = raw_rx.recv().await.unwrap().message else {
        panic!("expected SyncResponse");
    };
    assert_eq!(resp.files_updated, 1);
    assert_eq!(resp.files_deleted, 1);
    assert_eq!(resp.stale_paths, vec![failed_rel.to_string()]);

    let synced = workspace::synced_since(&root, snapshot_started);
    assert_eq!(
        synced.get(kept_rel),
        Some(&Some(content_hash(b"pub fn kept() -> u32 { 2 }\n")))
    );
    assert_eq!(synced.get(deleted_rel), Some(&None));
    assert!(
        !synced.contains_key(failed_rel),
        "failed session sync write must not be recorded as synced"
    );

    let restored = restore_after_lost_client(&manager, &root, before, snapshot_started).await;
    assert_eq!(restored, 2);
    assert_eq!(
        std::fs::read_to_string(root.join(kept_rel)).unwrap(),
        "pub fn kept() -> u32 { 2 }\n",
        "acknowledged session sync bytes must survive lost-client restore"
    );
    assert!(
        !root.join(deleted_rel).exists(),
        "acknowledged session sync deletion must survive lost-client restore"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(failed_rel)).unwrap(),
        "pub fn before_cmd() {}\n",
        "unacknowledged failed write must be restored from pre-exec snapshot"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(cmd_rel)).unwrap(),
        "pub fn cmd() -> u32 { 1 }\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_restore_transaction_restore_shares_sync_lock_across_write_to_record_and_cancellation()
{
    let storage = tempfile::tempdir().unwrap();
    let root = workspace::resolve_server_workspace(
        storage.path(),
        "/tmp/race-restore-ws",
        Some("race-restore-ws"),
    );
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"race_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub const V: u32 = 1;\n").unwrap();
    std::fs::write(root.join("src/cmd_only.rs"), "pub const CMD: u32 = 1;\n").unwrap();
    workspace::touch_last_used(&root);

    let engine = prod_code_engine_rust::RustEngine::load(&root).expect("load minimal rust engine");
    let engine_arc = Arc::new(tokio::sync::Mutex::new(engine));
    let ws = Arc::new(workspace::SharedWorkspace::new(
        root.clone(),
        "rust".to_string(),
        Some(Arc::clone(&engine_arc)),
        None,
        None,
        None,
    ));
    let manager = Arc::new(WorkspaceManager::new());
    manager.insert_ready_for_test(Arc::clone(&ws)).await;

    let snapshot_started = Instant::now();
    let before = Arc::new(snapshot_tree(&root));
    std::fs::write(root.join("src/cmd_only.rs"), "pub const CMD: u32 = 99;\n").unwrap();

    // Hold the engine lock so standalone apply_sync pauses inside push_base_updates after disk write.
    let held_engine = engine_arc.lock_owned().await;
    let sync_task = tokio::spawn({
        let storage_root = storage.path().to_path_buf();
        let manager = Arc::clone(&manager);
        async move {
            apply_sync(
                &storage_root,
                &manager,
                SyncRequest {
                    client_workspace_root: "/tmp/race-restore-ws".to_string(),
                    files: vec![write_delta("src/lib.rs", "pub const V: u32 = 2;\n")],
                    clean_others: false,
                    base_workspace_name: Some("race-restore-ws".to_string()),
                },
            )
            .await
        }
    });

    while std::fs::read_to_string(root.join("src/lib.rs"))
        .ok()
        .as_deref()
        != Some("pub const V: u32 = 2;\n")
    {
        tokio::task::yield_now().await;
    }

    let mut restore_task = tokio::spawn({
        let manager = Arc::clone(&manager);
        let root = root.clone();
        let before = Arc::clone(&before);
        async move { restore_after_lost_client(&manager, &root, before, snapshot_started).await }
    });

    let mut raced_during_sync = false;
    for _ in 0..2_000 {
        if std::fs::read_to_string(root.join("src/cmd_only.rs"))
            .ok()
            .as_deref()
            == Some("pub const CMD: u32 = 1;\n")
            || restore_task.is_finished()
        {
            raced_during_sync = true;
            break;
        }
        tokio::task::yield_now().await;
    }

    drop(held_engine);
    let sync_resp = sync_task.await.unwrap();
    let restored = (&mut restore_task).await.unwrap();

    assert!(
        !raced_during_sync,
        "restore_after_lost_client entered disk reconciliation while standalone sync held the workspace sync lock"
    );
    assert_eq!(sync_resp.files_updated, 1);
    assert_eq!(restored, 1);
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        "pub const V: u32 = 2;\n",
        "client sync written before restore must not be overwritten in the write-to-record interval"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/cmd_only.rs")).unwrap(),
        "pub const CMD: u32 = 1;\n"
    );

    // Verify cancelling the restore future after acquiring the lock retains ownership through disk work.
    std::fs::write(root.join("src/cmd_only.rs"), "pub const CMD: u32 = 777;\n").unwrap();
    let mut cancelled_restore = Box::pin(restore_after_lost_client(
        &manager,
        &root,
        Arc::clone(&before),
        snapshot_started,
    ));
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let _ = cancelled_restore.as_mut().poll(&mut cx);
    drop(cancelled_restore);

    let _after_guard = crate::sync::server_workspace_sync_lock(&root)
        .lock_owned()
        .await;
    assert_eq!(
        std::fs::read_to_string(root.join("src/cmd_only.rs")).unwrap(),
        "pub const CMD: u32 = 1;\n",
        "sync lock must stay held until blocking restore disk work finishes even if future is dropped"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        "pub const V: u32 = 2;\n"
    );
}
