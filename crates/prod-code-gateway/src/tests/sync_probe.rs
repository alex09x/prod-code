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
use prod_code_protocol::{FileDelta, FileStamp, SyncProbeRequest, SyncRequest};

#[tokio::test]
async fn test_delta_sync_reports_fresh_until_probed() {
    let storage = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let delta = || SyncRequest {
        client_workspace_root: "/tmp/ws".to_string(),
        files: vec![FileDelta {
            relative_path: "src/lib.rs".to_string(),
            content: Some(b"fn a() {}".to_vec()),
            is_executable: false,
        }],
        clean_others: false,
        base_workspace_name: Some("ws".to_string()),
    };
    let first = apply_sync(storage.path(), &manager, delta()).await;
    assert!(
        first.workspace_was_fresh,
        "nobody established this workspace yet"
    );
    let probe = apply_sync_probe(
        storage.path(),
        &manager,
        SyncProbeRequest {
            client_workspace_root: "/tmp/ws".to_string(),
            base_workspace_name: Some("ws".to_string()),
            seed_from: None,
            files: vec![FileStamp {
                relative_path: "src/lib.rs".to_string(),
                size: 9,
                hash: content_hash(b"fn a() {}"),
            }],
        },
    )
    .await;
    assert!(probe.missing.is_empty());
    let second = apply_sync(storage.path(), &manager, delta()).await;
    assert!(
        !second.workspace_was_fresh,
        "probe established the workspace"
    );
}

#[tokio::test]
async fn test_sync_probe_seeds_and_reconciles() {
    let storage = tempfile::tempdir().unwrap();
    let origin = storage.path().join("repo");
    std::fs::create_dir_all(origin.join("src")).unwrap();
    std::fs::write(origin.join("Cargo.toml"), "[package]\nname = \"repo\"\n").unwrap();
    std::fs::write(origin.join("src/lib.rs"), "pub fn a() {}").unwrap();
    std::fs::write(origin.join("src/only_in_origin.rs"), "pub fn gone() {}").unwrap();
    let manager = WorkspaceManager::new();

    let req = SyncProbeRequest {
        client_workspace_root: "/tmp/wt".to_string(),
        base_workspace_name: Some("repo--wt-0001".to_string()),
        seed_from: Some("repo".to_string()),
        files: vec![
            FileStamp {
                relative_path: "Cargo.toml".to_string(),
                size: 24,
                hash: content_hash(b"[package]\nname = \"repo\"\n"),
            },
            FileStamp {
                relative_path: "src/lib.rs".to_string(),
                size: 21,
                hash: content_hash(b"pub fn a() -> u8 {}"),
            },
            FileStamp {
                relative_path: "src/new.rs".to_string(),
                size: 3,
                hash: content_hash(b"// n"),
            },
        ],
    };
    let resp = apply_sync_probe(storage.path(), &manager, req).await;
    assert!(resp.seeded);
    assert_eq!(resp.files_deleted, 1);
    assert_eq!(
        resp.missing,
        vec!["src/lib.rs".to_string(), "src/new.rs".to_string()]
    );
    let wt = storage.path().join("repo--wt-0001");
    assert!(wt.join("Cargo.toml").exists());
    assert!(!wt.join("src/only_in_origin.rs").exists());
    assert!(
        origin.join("src/only_in_origin.rs").exists(),
        "origin copy untouched"
    );

    // A second probe on the now-populated directory does not seed again.
    let again = apply_sync_probe(
        storage.path(),
        &manager,
        SyncProbeRequest {
            client_workspace_root: "/tmp/wt".to_string(),
            base_workspace_name: Some("repo--wt-0001".to_string()),
            seed_from: Some("repo".to_string()),
            files: vec![FileStamp {
                relative_path: "Cargo.toml".to_string(),
                size: 24,
                hash: content_hash(b"[package]\nname = \"repo\"\n"),
            }],
        },
    )
    .await;
    assert!(!again.seeded);
    assert!(again.missing.is_empty());
    assert_eq!(
        again.files_deleted, 1,
        "src/lib.rs is not in the manifest any more"
    );
}

#[tokio::test]
async fn test_typescript_type_roots_survive_manifest_reconciliation_and_upload() {
    let storage = tempfile::tempdir().unwrap();
    let origin = storage.path().join("typescript-repo");
    std::fs::create_dir_all(&origin).unwrap();
    let package = b"{}\n";
    let tsconfig = b"{\"compilerOptions\":{\"typeRoots\":[\"custom_types\"]}}";
    std::fs::write(origin.join("package.json"), package).unwrap();
    std::fs::write(origin.join("tsconfig.json"), tsconfig).unwrap();

    let response = apply_sync_probe(
        storage.path(),
        &WorkspaceManager::new(),
        SyncProbeRequest {
            client_workspace_root: "/tmp/typescript-worktree".to_string(),
            base_workspace_name: Some("typescript-repo--wt-0001".to_string()),
            seed_from: Some("typescript-repo".to_string()),
            files: vec![
                FileStamp {
                    relative_path: "package.json".to_string(),
                    size: package.len() as u64,
                    hash: content_hash(package),
                },
                FileStamp {
                    relative_path: "tsconfig.json".to_string(),
                    size: tsconfig.len() as u64,
                    hash: content_hash(tsconfig),
                },
            ],
        },
    )
    .await;

    assert!(!response.missing.iter().any(|path| path == "tsconfig.json"));
    let workspace = storage.path().join("typescript-repo--wt-0001");
    let coordinated = std::fs::read_to_string(workspace.join("tsconfig.json")).unwrap();
    assert!(coordinated.contains("node_modules/@types"));

    crate::sync::sync_fs::write_synced_file(&workspace.join("tsconfig.json"), tsconfig, false)
        .await
        .unwrap();
    let after_upload = std::fs::read_to_string(workspace.join("tsconfig.json")).unwrap();
    assert!(after_upload.contains("node_modules/@types"));
}
