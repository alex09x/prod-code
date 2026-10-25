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

#[tokio::test]
pub(super) async fn sync_restore_transaction_failed_deletes_stay_stale_while_absent_deletes_succeed()
 {
    let storage = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let stale_seed = [
        "src/dir_slot".to_string(),
        "src/already_absent.rs".to_string(),
        "src/removable.rs".to_string(),
    ];
    let deltas = || {
        vec![
            delete_delta("src/removable.rs"),
            delete_delta("src/already_absent.rs"),
            delete_delta("src/dir_slot"),
        ]
    };

    // 1. Standalone apply_sync deletion branch.
    let standalone_root = workspace::resolve_server_workspace(
        storage.path(),
        "/tmp/delete-standalone",
        Some("delete-standalone"),
    );
    std::fs::create_dir_all(standalone_root.join("src/dir_slot/nested")).unwrap();
    std::fs::write(
        standalone_root.join("src/removable.rs"),
        "pub fn removable() {}\n",
    )
    .unwrap();
    workspace::record_stale_paths(&standalone_root, &stale_seed);

    let standalone_start = Instant::now();
    let standalone_resp = apply_sync(
        storage.path(),
        &manager,
        SyncRequest {
            client_workspace_root: "/tmp/delete-standalone".to_string(),
            files: deltas(),
            clean_others: false,
            base_workspace_name: Some("delete-standalone".to_string()),
        },
    )
    .await;

    assert_eq!(standalone_resp.files_deleted, 1);
    assert_eq!(
        standalone_resp.stale_paths,
        vec!["src/dir_slot".to_string()],
        "standalone sync must report failed deletion stale and clear normal/already-absent deletions"
    );
    assert_eq!(
        workspace::stale_paths(&standalone_root),
        vec!["src/dir_slot".to_string()]
    );
    let standalone_synced = workspace::synced_since(&standalone_root, standalone_start);
    assert_eq!(standalone_synced.get("src/removable.rs"), Some(&None));
    assert_eq!(standalone_synced.get("src/already_absent.rs"), Some(&None));
    assert!(!standalone_synced.contains_key("src/dir_slot"));

    // 2. In-session handle_session_sync deletion branch.
    let session_root = workspace::resolve_server_workspace(
        storage.path(),
        "/tmp/delete-session",
        Some("delete-session"),
    );
    std::fs::create_dir_all(session_root.join("src/dir_slot/nested")).unwrap();
    std::fs::write(
        session_root.join("src/removable.rs"),
        "pub fn removable() {}\n",
    )
    .unwrap();
    workspace::record_stale_paths(&session_root, &stale_seed);

    let (view, out_tx, mut raw_rx) = make_session_view(&manager, &session_root, 2).await;
    let session_start = Instant::now();
    crate::session::handle_session_sync(
        SyncRequest {
            client_workspace_root: "/tmp/delete-session".to_string(),
            files: deltas(),
            clean_others: false,
            base_workspace_name: Some("delete-session".to_string()),
        },
        &view,
        &out_tx,
    )
    .await;

    let WireMessage::SyncResponse(session_resp) = raw_rx.recv().await.unwrap().message else {
        panic!("expected SyncResponse");
    };
    assert_eq!(session_resp.files_deleted, 1);
    assert_eq!(
        session_resp.stale_paths,
        vec!["src/dir_slot".to_string()],
        "session sync must report failed deletion stale and clear normal/already-absent deletions"
    );
    assert_eq!(
        workspace::stale_paths(&session_root),
        vec!["src/dir_slot".to_string()]
    );
    let session_synced = workspace::synced_since(&session_root, session_start);
    assert_eq!(session_synced.get("src/removable.rs"), Some(&None));
    assert_eq!(session_synced.get("src/already_absent.rs"), Some(&None));
    assert!(!session_synced.contains_key("src/dir_slot"));
}
