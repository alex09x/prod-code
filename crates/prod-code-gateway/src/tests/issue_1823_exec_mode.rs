/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1823: the node copy keeps a synced file's executable bit, a mode-only update reaches it,
//! and a probe asks again for a copy whose bytes match but whose bit does not.
#![cfg(unix)]

use super::super::*;
use prod_code_protocol::{FileDelta, FileStamp, SyncProbeRequest, SyncRequest};
use std::os::unix::fs::PermissionsExt;

const SCRIPT: &[u8] = b"#!/usr/bin/env python3\nprint('hi')\n";

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn upload(files: Vec<FileDelta>) -> SyncRequest {
    SyncRequest {
        client_workspace_root: "/tmp/ws".to_string(),
        files,
        clean_others: false,
        base_workspace_name: Some("ws".to_string()),
    }
}

fn file(path: &str, content: &[u8], is_executable: bool) -> FileDelta {
    FileDelta {
        relative_path: path.to_string(),
        content: Some(content.to_vec()),
        is_executable,
    }
}

#[tokio::test]
async fn issue_1823_executable_upload_and_mode_only_update_reach_the_node() {
    let storage = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let root = storage.path().join("ws");
    let first = apply_sync(
        storage.path(),
        &manager,
        upload(vec![
            file("tool.py", SCRIPT, true),
            file("lib.py", b"VALUE = 1\n", false),
        ]),
    )
    .await;
    assert_eq!(first.files_updated, 2);
    assert_eq!(mode(&root.join("tool.py")), 0o755);
    assert_eq!(
        mode(&root.join("lib.py")) & 0o111,
        0,
        "a plain file stays plain"
    );

    // Same bytes, other bit, both ways: a mode-only delta is applied, never skipped.
    let flip = apply_sync(
        storage.path(),
        &manager,
        upload(vec![
            file("tool.py", SCRIPT, false),
            file("lib.py", b"VALUE = 1\n", true),
        ]),
    )
    .await;
    assert_eq!(flip.files_updated, 2);
    assert_eq!(mode(&root.join("tool.py")) & 0o111, 0);
    assert_eq!(mode(&root.join("lib.py")), 0o755);
    assert_eq!(std::fs::read(root.join("tool.py")).unwrap(), SCRIPT);
}

#[tokio::test]
async fn issue_1823_probe_requests_a_copy_with_the_wrong_executable_bit() {
    let storage = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let root = storage.path().join("ws");
    apply_sync(
        storage.path(),
        &manager,
        upload(vec![
            file("stale.py", SCRIPT, false),
            file("good.py", SCRIPT, true),
            file("plain.py", b"X = 1\n", false),
            file("legacy.py", SCRIPT, false),
        ]),
    )
    .await;
    let stamp = |rel: &str, content: &[u8], is_executable| FileStamp {
        relative_path: rel.to_string(),
        size: content.len() as u64,
        hash: content_hash(content),
        is_executable,
    };
    let probe = apply_sync_probe(
        storage.path(),
        &manager,
        SyncProbeRequest {
            client_workspace_root: "/tmp/ws".to_string(),
            base_workspace_name: Some("ws".to_string()),
            seed_from: None,
            files: vec![
                stamp("stale.py", SCRIPT, Some(true)),
                stamp("good.py", SCRIPT, Some(true)),
                stamp("plain.py", b"X = 1\n", Some(false)),
                // An older client sends no bit: judged by content, as before.
                stamp("legacy.py", SCRIPT, None),
            ],
        },
    )
    .await;
    assert_eq!(probe.missing, ["stale.py"]);
    assert_eq!(probe.files_deleted, 0);

    apply_sync(
        storage.path(),
        &manager,
        upload(vec![file("stale.py", SCRIPT, true)]),
    )
    .await;
    assert_eq!(mode(&root.join("stale.py")), 0o755);
}

/// The executable bit never widens what a sync may write: escaping paths stay refused, and
/// no setuid, setgid or sticky bit is ever set, whatever the client claims.
#[tokio::test]
async fn issue_1823_executable_bit_does_not_unlock_unsafe_paths() {
    let storage = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let root = storage.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("linked")).unwrap();
    let response = apply_sync(
        storage.path(),
        &manager,
        upload(vec![
            file("../escape.py", SCRIPT, true),
            file("linked/escape.py", SCRIPT, true),
            file("ok.py", SCRIPT, true),
        ]),
    )
    .await;
    assert_eq!(response.files_updated, 1);
    for rejected in ["../escape.py", "linked/escape.py"] {
        assert!(response.stale_paths.iter().any(|p| p == rejected));
    }
    assert!(!storage.path().join("escape.py").exists());
    assert!(!outside.path().join("escape.py").exists());
    assert_eq!(mode(&root.join("ok.py")), 0o755, "no special bits");
}
