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

#[cfg(unix)]
#[tokio::test]
async fn session_sync_rejects_absolute_parent_and_symlink_paths() {
    use std::os::unix::fs::symlink;

    let storage = tempfile::tempdir().unwrap();
    let outside = storage.path();
    let root = storage.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    symlink(outside, root.join("linked")).unwrap();

    let victim = outside.join("victim.txt");
    std::fs::write(&victim, b"keep").unwrap();
    let absolute_write = outside.join("absolute-escape.txt");
    let parent_write = storage.path().join("parent-escape.txt");
    let symlink_write = outside.join("symlink-escape.txt");

    let workspace = Arc::new(workspace::SharedWorkspace::new(
        root.clone(),
        "generic-lsp".to_string(),
        None,
        None,
        None,
        None,
    ));
    let manager = workspace::WorkspaceManager::new();
    let lease = workspace::WorkspaceLease::acquire(workspace);
    let view = manager.register_session_view(1, root, lease).await;
    let (raw_tx, mut raw_rx) = rapidfire::mpsc::bounded(8);
    let out_tx = SharedOutputSender::new(raw_tx, std::time::Duration::from_secs(1));

    handle_session_sync(
        SyncRequest {
            client_workspace_root: "/tmp/ws".to_string(),
            files: vec![
                FileDelta {
                    relative_path: "../parent-escape.txt".to_string(),
                    content: Some(b"parent".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: absolute_write.to_string_lossy().into_owned(),
                    content: Some(b"absolute".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: "linked/symlink-escape.txt".to_string(),
                    content: Some(b"symlink".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: "../victim.txt".to_string(),
                    content: None,
                    is_executable: false,
                },
            ],
            clean_others: false,
            base_workspace_name: Some("ws".to_string()),
        },
        &view,
        &out_tx,
    )
    .await;

    let response = raw_rx.recv().await.unwrap().message;
    let WireMessage::SyncResponse(response) = response else {
        panic!("expected sync response");
    };
    assert_eq!(response.files_updated, 0);
    assert_eq!(response.files_deleted, 0);
    for rejected in [
        "../parent-escape.txt",
        absolute_write.to_str().unwrap(),
        "linked/symlink-escape.txt",
        "../victim.txt",
    ] {
        assert!(
            response.stale_paths.iter().any(|path| path == rejected),
            "invalid path should be retried: {rejected:?}; stale paths: {:?}",
            response.stale_paths
        );
    }
    assert!(!parent_write.exists());
    assert!(!absolute_write.exists());
    assert!(!symlink_write.exists());
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
}

fn delta(path: &str, content: Option<&str>, is_executable: bool) -> FileDelta {
    FileDelta {
        relative_path: path.to_string(),
        content: content.map(|c| c.as_bytes().to_vec()),
        is_executable,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn session_sync_writes_deletes_and_reports_what_it_could_not_write() {
    let storage = tempfile::tempdir().unwrap();
    let root = storage.path().join("ws");
    for (rel, body) in [
        ("exists.txt", "old"),
        ("gone.txt", "bye"),
        ("old/dir/gone.txt", "x"),
    ] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    std::fs::create_dir_all(root.join("blocked")).unwrap();
    let workspace = Arc::new(workspace::SharedWorkspace::new(
        root.clone(),
        "generic-lsp".to_string(),
        None,
        None,
        None,
        None,
    ));
    let manager = workspace::WorkspaceManager::new();
    let lease = workspace::WorkspaceLease::acquire(workspace);
    let view = manager.register_session_view(1, root.clone(), lease).await;
    let (raw_tx, mut raw_rx) = rapidfire::mpsc::bounded(8);
    let out_tx = SharedOutputSender::new(raw_tx, std::time::Duration::from_secs(1));

    handle_session_sync(
        SyncRequest {
            client_workspace_root: "/tmp/ws".to_string(),
            files: vec![
                delta("new/dir/created.txt", Some("fresh"), false),
                delta("exists.txt", Some("newer text"), false),
                delta("run.sh", Some("#!/bin/sh\n"), true),
                delta("blocked", Some("a file where a directory is"), false),
                delta("gone.txt", None, false),
                delta("old/dir/gone.txt", None, false),
                delta("never-there.txt", None, false),
            ],
            clean_others: true,
            base_workspace_name: Some("ws".to_string()),
        },
        &view,
        &out_tx,
    )
    .await;

    let WireMessage::SyncResponse(response) = raw_rx.recv().await.unwrap().message else {
        panic!("expected sync response");
    };
    assert_eq!(response.files_updated, 3);
    assert_eq!(response.files_deleted, 2);
    assert_eq!(
        response.bytes_transferred,
        "fresh".len()
            + "newer text".len()
            + "#!/bin/sh\n".len()
            + "a file where a directory is".len()
    );
    assert_eq!(response.stale_paths, ["blocked"]);
    assert_eq!(response.server_workspace_root, root.to_string_lossy());
    assert!(!response.workspace_was_fresh);
    assert_eq!(
        std::fs::read_to_string(root.join("new/dir/created.txt")).unwrap(),
        "fresh"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("exists.txt")).unwrap(),
        "newer text"
    );
    assert!(
        root.join("blocked").is_dir(),
        "the directory was not replaced"
    );
    assert!(!root.join("gone.txt").exists());
    assert!(
        !root.join("old").exists(),
        "the emptied directories are pruned too"
    );
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(root.join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111);
}

#[tokio::test]
async fn syncing_a_project_manifest_retires_the_session_after_its_response() {
    let storage = tempfile::tempdir().unwrap();
    let root = storage.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let workspace = Arc::new(workspace::SharedWorkspace::new(
        root.clone(),
        "rust".to_string(),
        None,
        None,
        None,
        None,
    ));
    let mut invalidation = workspace.rebalance_tx.subscribe();
    let manager = workspace::WorkspaceManager::new();
    let lease = workspace::WorkspaceLease::acquire(workspace);
    let view = manager.register_session_view(1, root, lease).await;
    let (raw_tx, mut raw_rx) = rapidfire::mpsc::bounded(8);
    let out_tx = SharedOutputSender::new(raw_tx, std::time::Duration::from_secs(1));

    handle_session_sync(
        SyncRequest {
            client_workspace_root: "/tmp/ws".to_string(),
            files: vec![delta(
                "Cargo.toml",
                Some("[package]\nname = \"ws\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
                false,
            )],
            clean_others: false,
            base_workspace_name: Some("ws".to_string()),
        },
        &view,
        &out_tx,
    )
    .await;

    assert!(matches!(
        raw_rx.recv().await.unwrap().message,
        WireMessage::SyncResponse(_)
    ));
    assert!(view.accounted.has_stale_manifests());
    assert_eq!(
        invalidation.recv().await.unwrap().1.as_deref(),
        Some(prod_code_protocol::WORKSPACE_MODEL_CHANGED_REASON)
    );
}
