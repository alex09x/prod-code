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
use prod_code_protocol::{FileDelta, SyncRequest};
use std::sync::Arc;
use std::time::Instant;

#[test]
fn a_sync_is_remembered_per_workspace_from_the_time_it_lands() {
    let one = std::path::Path::new("/nonexistent/sync-log-one");
    let other = std::path::Path::new("/nonexistent/sync-log-other");
    let start = Instant::now();
    workspace::record_synced(
        one,
        &[("a.rs".to_string(), Some(7)), ("gone.rs".to_string(), None)],
    );
    let synced = workspace::synced_since(one, start);
    assert_eq!(synced.get("a.rs"), Some(&Some(7)));
    assert_eq!(synced.get("gone.rs"), Some(&None));
    assert!(workspace::synced_since(other, start).is_empty());
    assert!(workspace::synced_since(one, Instant::now()).is_empty());
    workspace::record_synced(one, &[]);
}

/// A file whose old bytes did not fit the snapshot's limits cannot be put back: it leaves the
/// copy and is reported stale by every sync answer until the client has sent it (#262).
#[tokio::test]
async fn a_file_past_the_snapshot_limits_is_removed_and_reported_stale() {
    let storage = tempfile::tempdir().unwrap();
    let root = storage.path().join("ws");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "aaaa").unwrap();
    std::fs::write(root.join("src/b.rs"), "bbbb").unwrap();
    std::fs::write(root.join("src/big.rs"), "0123456789").unwrap();
    // Files are kept in path order: a.rs fits, b.rs is over the budget a.rs leaves, and
    // big.rs is over the per-file limit.
    let before = snapshot_tree_within(&root, 8, 6);
    assert_eq!(before.kept.keys().collect::<Vec<_>>(), ["src/a.rs"]);

    std::fs::write(root.join("src/a.rs"), "changed").unwrap();
    std::fs::write(root.join("src/b.rs"), "changed").unwrap();
    std::fs::remove_file(root.join("src/big.rs")).unwrap();
    let manager = WorkspaceManager::new();
    let restored =
        restore_after_lost_client(&manager, &root, Arc::new(before), Instant::now()).await;
    assert_eq!(restored, 1);
    assert_eq!(
        std::fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "aaaa"
    );
    assert!(
        !root.join("src/b.rs").exists(),
        "a changed file not kept leaves"
    );
    assert_eq!(
        workspace::stale_paths(&root),
        ["src/b.rs".to_string(), "src/big.rs".to_string()]
    );

    // The next sync answers with what it still lacks until the client has sent both.
    let sync = |files: Vec<FileDelta>| SyncRequest {
        client_workspace_root: "/tmp/ws".to_string(),
        files,
        clean_others: false,
        base_workspace_name: Some("ws".to_string()),
    };
    let first = apply_sync(
        storage.path(),
        &manager,
        sync(vec![
            FileDelta {
                relative_path: "src/a.rs".to_string(),
                content: Some(b"aaaa".to_vec()),
                is_executable: false,
            },
            FileDelta {
                relative_path: "src/b.rs".to_string(),
                content: Some(b"bbbb".to_vec()),
                is_executable: false,
            },
        ]),
    )
    .await;
    assert_eq!(first.stale_paths, ["src/big.rs".to_string()]);
    let second = apply_sync(
        storage.path(),
        &manager,
        sync(vec![FileDelta {
            relative_path: "src/big.rs".to_string(),
            content: None,
            is_executable: false,
        }]),
    )
    .await;
    assert!(second.stale_paths.is_empty());
    assert!(!root.join(workspace::STALE_MARKER).exists());
}

/// A file the copy cannot take keeps its old text, is not counted, and comes back stale until
/// the client has sent it again. A read-only directory stands in for a full disk, where
/// `fs::write` truncated the file and reported nothing (#385).
#[tokio::test]
async fn a_sync_write_that_fails_keeps_the_old_text_and_asks_for_it_again() {
    use std::os::unix::fs::PermissionsExt;
    let storage = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let sync = |text: &str| SyncRequest {
        client_workspace_root: "/tmp/ws".to_string(),
        files: vec![FileDelta {
            relative_path: "src/lib.rs".to_string(),
            content: Some(text.as_bytes().to_vec()),
            is_executable: false,
        }],
        clean_others: false,
        base_workspace_name: Some("ws".to_string()),
    };
    apply_sync(storage.path(), &manager, sync("fn old() {}")).await;
    let root = storage.path().join("ws");
    let src = root.join("src");
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o555)).unwrap();
    let refused = apply_sync(storage.path(), &manager, sync("fn new() {}")).await;
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        std::fs::read_to_string(src.join("lib.rs")).unwrap(),
        "fn old() {}"
    );
    assert_eq!(refused.files_updated, 0);
    assert_eq!(refused.stale_paths, ["src/lib.rs".to_string()]);
    assert_eq!(workspace::stale_paths(&root), ["src/lib.rs".to_string()]);
    assert!(
        std::fs::read_dir(&src).unwrap().count() == 1,
        "no temporary file is left behind"
    );

    let again = apply_sync(storage.path(), &manager, sync("fn new() {}")).await;
    assert_eq!(again.files_updated, 1);
    assert!(again.stale_paths.is_empty());
    assert_eq!(
        std::fs::read_to_string(src.join("lib.rs")).unwrap(),
        "fn new() {}"
    );
}
