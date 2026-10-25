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

async fn coordinated_config_survives_restore(session: bool) {
    let storage = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let root = workspace::resolve_server_workspace(storage.path(), "/tmp/config", Some("config"));
    std::fs::create_dir_all(&root).unwrap();
    for name in ["tsconfig.json", "jsconfig.json"] {
        std::fs::write(
            root.join(name),
            r#"{"compilerOptions":{"typeRoots":["old"]}}"#,
        )
        .unwrap();
    }
    std::fs::create_dir_all(root.join("blocked/tsconfig.json")).unwrap();
    std::fs::write(root.join("blocked/tsconfig.json/keep.txt"), "keep\n").unwrap();
    let started = Instant::now();
    let before = Arc::new(snapshot_tree(&root));
    let payload = r#"{"compilerOptions":{"typeRoots":["./custom"]}}"#;
    let req = SyncRequest {
        client_workspace_root: "/tmp/config".into(),
        base_workspace_name: Some("config".into()),
        files: ["tsconfig.json", "jsconfig.json", "blocked/tsconfig.json"]
            .map(|name| write_delta(name, payload))
            .to_vec(),
        clean_others: false,
    };
    let response = if session {
        let (view, out, mut receiver) = make_session_view(&manager, &root, 1).await;
        tokio::time::timeout(
            Duration::from_secs(10),
            crate::session::handle_session_sync(req, &view, &out),
        )
        .await
        .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        let WireMessage::SyncResponse(response) = frame.message else {
            panic!("expected sync response")
        };
        response
    } else {
        tokio::time::timeout(
            Duration::from_secs(10),
            apply_sync(storage.path(), &manager, req),
        )
        .await
        .unwrap()
    };
    assert_eq!(response.files_updated, 2);
    assert_eq!(response.stale_paths, vec!["blocked/tsconfig.json"]);
    assert!(
        !workspace::synced_since(&root, started).contains_key("blocked/tsconfig.json"),
        "failed config writes must remain unrecorded"
    );
    let committed: Vec<_> = ["tsconfig.json", "jsconfig.json"]
        .map(|name| {
            let bytes = std::fs::read(root.join(name)).unwrap();
            assert_ne!(
                bytes,
                payload.as_bytes(),
                "fixture must exercise config coordination"
            );
            (name, bytes)
        })
        .to_vec();
    tokio::time::timeout(
        Duration::from_secs(10),
        restore_after_lost_client(&manager, &root, before, started),
    )
    .await
    .unwrap();
    for (name, bytes) in committed {
        assert_eq!(
            std::fs::read(root.join(name)).ok(),
            Some(bytes.clone()),
            "acknowledged coordinated {name} must survive restore"
        );
        assert_eq!(
            workspace::synced_since(&root, started).get(name),
            Some(&Some(content_hash(&bytes)))
        );
    }
    assert_eq!(workspace::stale_paths(&root), vec!["blocked/tsconfig.json"]);
    assert_eq!(
        std::fs::read_to_string(root.join("blocked/tsconfig.json/keep.txt")).unwrap(),
        "keep\n"
    );
}

#[tokio::test]
async fn sync_restore_transaction_review_session_config_hash() {
    coordinated_config_survives_restore(true).await;
}

#[tokio::test]
async fn sync_restore_transaction_review_standalone_config_hash() {
    coordinated_config_survives_restore(false).await;
}
