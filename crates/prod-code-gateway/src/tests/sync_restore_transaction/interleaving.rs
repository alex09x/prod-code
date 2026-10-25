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
use std::future::Future;
use std::pin::Pin;

async fn cancelled_sync_records_before_restore(session: bool) {
    let storage = tempfile::tempdir().unwrap();
    let root =
        workspace::resolve_server_workspace(storage.path(), "/tmp/interleave", Some("interleave"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("client.rs"), "old client\n").unwrap();
    std::fs::write(root.join("command.rs"), "old command\n").unwrap();
    let started = Instant::now();
    let before = Arc::new(snapshot_tree(&root));
    std::fs::write(root.join("command.rs"), "command dirty\n").unwrap();
    let manager = WorkspaceManager::new();
    let (view, out, _receiver) = make_session_view(&manager, &root, 1).await;
    let req = SyncRequest {
        client_workspace_root: "/tmp/interleave".into(),
        base_workspace_name: Some("interleave".into()),
        files: vec![write_delta("client.rs", "new client\n")],
        clean_others: false,
    };
    let hook = Installation::new(&root, Point::PostWrite);
    let mut caller: Pin<Box<dyn Future<Output = ()> + Send + '_>> = if session {
        Box::pin(async { crate::session::handle_session_sync(req, &view, &out).await })
    } else {
        Box::pin(async {
            apply_sync(storage.path(), &manager, req).await;
        })
    };
    tokio::select! {
        _ = hook.entered() => {},
        _ = &mut caller => panic!("sync returned before post-write barrier"),
    }
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(caller.as_mut().poll(&mut cx).is_pending());
    assert_eq!(
        std::fs::read_to_string(root.join("client.rs")).unwrap(),
        "new client\n"
    );
    assert!(
        workspace::synced_since(&root, started).is_empty(),
        "barrier must precede record_synced"
    );
    drop(caller);

    let mut restore = Box::pin(restore_after_lost_client(&manager, &root, before, started));
    assert!(
        restore.as_mut().poll(&mut cx).is_pending(),
        "restore must wait for cancelled sync's disk worker"
    );
    assert!(
        crate::sync::server_workspace_sync_lock(&root)
            .try_lock()
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(root.join("command.rs")).unwrap(),
        "command dirty\n"
    );
    hook.release();
    let restored = tokio::time::timeout(Duration::from_secs(10), restore)
        .await
        .unwrap();
    assert_eq!(restored, 1);
    assert_eq!(
        std::fs::read_to_string(root.join("client.rs")).unwrap(),
        "new client\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("command.rs")).unwrap(),
        "old command\n"
    );
    assert_eq!(
        workspace::synced_since(&root, started).get("client.rs"),
        Some(&Some(content_hash(b"new client\n")))
    );
}

#[tokio::test]
async fn sync_restore_transaction_post_write_standalone_cancellation() {
    cancelled_sync_records_before_restore(false).await;
}

#[tokio::test]
async fn sync_restore_transaction_post_write_session_cancellation() {
    cancelled_sync_records_before_restore(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_restore_transaction_blocking_restore_cancellation() {
    let storage = tempfile::tempdir().unwrap();
    let root = workspace::resolve_server_workspace(storage.path(), "/tmp/cancel", Some("cancel"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("command.rs"), "old command\n").unwrap();
    std::fs::write(root.join("unkept.rs"), "unkept\n").unwrap();
    let started = Instant::now();
    let mut snapshot = snapshot_tree(&root);
    snapshot.kept.remove("unkept.rs");
    std::fs::write(root.join("command.rs"), "command dirty\n").unwrap();
    std::fs::write(root.join("unkept.rs"), "dirty unkept\n").unwrap();
    let manager = WorkspaceManager::new();
    let hook = Installation::new(&root, Point::RestoreDisk);
    let mut caller = Box::pin(restore_after_lost_client(
        &manager,
        &root,
        Arc::new(snapshot),
        started,
    ));
    tokio::select! {
        _ = hook.entered() => {},
        _ = &mut caller => panic!("restore returned before worker barrier"),
    }
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(caller.as_mut().poll(&mut cx).is_pending());
    drop(caller);
    let mut sync = Box::pin(apply_sync(
        storage.path(),
        &manager,
        SyncRequest {
            client_workspace_root: "/tmp/cancel".into(),
            base_workspace_name: Some("cancel".into()),
            files: vec![write_delta("client.rs", "new client\n")],
            clean_others: false,
        },
    ));
    assert!(
        sync.as_mut().poll(&mut cx).is_pending(),
        "sync must wait for cancelled blocking restore"
    );
    assert!(
        crate::sync::server_workspace_sync_lock(&root)
            .try_lock()
            .is_err()
    );
    assert!(!root.join("client.rs").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("command.rs")).unwrap(),
        "command dirty\n"
    );
    hook.release();
    let response = tokio::time::timeout(Duration::from_secs(10), sync)
        .await
        .unwrap();
    assert_eq!(response.files_updated, 1);
    assert_eq!(
        std::fs::read_to_string(root.join("command.rs")).unwrap(),
        "old command\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("client.rs")).unwrap(),
        "new client\n"
    );
    assert!(!root.join("unkept.rs").exists());
    assert_eq!(workspace::stale_paths(&root), vec!["unkept.rs"]);
    assert_eq!(
        workspace::synced_since(&root, started).get("client.rs"),
        Some(&Some(content_hash(b"new client\n")))
    );
}
