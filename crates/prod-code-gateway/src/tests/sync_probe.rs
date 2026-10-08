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

    let request = |config: &[u8], seed_from: Option<&str>, workspace_name: &str| SyncProbeRequest {
        client_workspace_root: "/tmp/typescript-worktree".to_string(),
        base_workspace_name: Some(workspace_name.to_string()),
        seed_from: seed_from.map(str::to_string),
        files: vec![
            FileStamp {
                relative_path: "package.json".to_string(),
                size: package.len() as u64,
                hash: content_hash(package),
            },
            FileStamp {
                relative_path: "tsconfig.json".to_string(),
                size: config.len() as u64,
                hash: content_hash(config),
            },
        ],
    };

    let response = apply_sync_probe(
        storage.path(),
        &WorkspaceManager::new(),
        request(
            tsconfig,
            Some("typescript-repo"),
            "typescript-repo--wt-0001",
        ),
    )
    .await;

    assert!(!response.missing.iter().any(|path| path == "tsconfig.json"));
    let workspace = storage.path().join("typescript-repo--wt-0001");
    let coordinated = std::fs::read_to_string(workspace.join("tsconfig.json")).unwrap();
    assert!(coordinated.contains("node_modules/@types"));

    crate::sync::file_write::write_synced_file(
        &workspace,
        &workspace.join("tsconfig.json"),
        tsconfig,
        false,
    )
    .await
    .unwrap();
    let after_upload = std::fs::read_to_string(workspace.join("tsconfig.json")).unwrap();
    assert!(after_upload.contains("node_modules/@types"));
    let parsed: serde_json::Value = serde_json::from_str(&after_upload).unwrap();
    assert_eq!(
        parsed["compilerOptions"]["typeRoots"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let repeated = apply_sync_probe(
        storage.path(),
        &WorkspaceManager::new(),
        request(tsconfig, None, "typescript-repo--wt-0001"),
    )
    .await;
    assert!(repeated.missing.is_empty());

    let changed_config =
        b"{\"compilerOptions\":{\"typeRoots\":[\"custom_types\"],\"strict\":true}}";
    let changed = apply_sync_probe(
        storage.path(),
        &WorkspaceManager::new(),
        request(changed_config, None, "typescript-repo--wt-0001"),
    )
    .await;
    assert!(changed.missing.iter().any(|path| path == "tsconfig.json"));

    crate::sync::file_write::write_synced_file(
        &origin,
        &origin.join("tsconfig.json"),
        tsconfig,
        false,
    )
    .await
    .unwrap();
    let new_worktree = apply_sync_probe(
        storage.path(),
        &WorkspaceManager::new(),
        request(
            tsconfig,
            Some("typescript-repo"),
            "typescript-repo--wt-0002",
        ),
    )
    .await;
    assert!(new_worktree.missing.is_empty());
    let seeded_config = storage
        .path()
        .join("typescript-repo--wt-0002/tsconfig.json");
    assert!(
        std::fs::read_to_string(seeded_config)
            .unwrap()
            .contains("node_modules/@types")
    );
}

#[tokio::test]
async fn test_typescript_config_remains_complete_when_stamp_write_fails() {
    let storage = tempfile::tempdir().unwrap();
    let workspace = storage.path().join("typescript-repo");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        storage.path().join(crate::sync::config_meta::METADATA_DIR),
        b"block metadata directory",
    )
    .unwrap();

    let config = workspace.join("tsconfig.json");
    let content = b"{\"compilerOptions\":{\"typeRoots\":[\"custom_types\"]}}";
    let result =
        crate::sync::file_write::write_synced_file(&workspace, &config, content, false).await;
    assert!(result.is_err());

    let stored = std::fs::read(&config).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&stored).unwrap();
    assert!(
        parsed["compilerOptions"]["typeRoots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|root| root == "node_modules/@types")
    );
}
