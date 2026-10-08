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
use std::time::Instant;

#[tokio::test]
async fn test_server_state_status() {
    let temp = tempfile::tempdir().unwrap();
    let state = ServerState::new(temp.path().to_path_buf());
    let status = state.status().await;
    assert_eq!(status.server_pid, std::process::id());
    assert_eq!(status.active_sessions, 0);
    assert_eq!(status.loaded_workspaces, 0);
    assert!(
        status
            .detected_engines
            .contains(&"rust (ra_ap_ide)".to_string())
    );
    assert!(state.serves_engine("rust") && state.serves_engine("swift"));
}

#[tokio::test]
async fn test_engine_allowlist_narrows_advertised_engines() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = ServerState::new(temp.path().to_path_buf());
    state.engine_allowlist = vec!["swift".to_string()];
    let status = state.status().await;
    assert!(
        !status
            .detected_engines
            .iter()
            .any(|e| e.starts_with("rust") || e == "generic-lsp"),
        "{:?}",
        status.detected_engines
    );
    assert!(
        status
            .detected_engines
            .iter()
            .all(|e| e.starts_with("swift"))
    );
    assert!(state.serves_engine("swift") && state.serves_engine("Swift"));
    assert!(!state.serves_engine("rust") && !state.serves_engine("generic"));
}

#[test]
fn test_engine_allowlist_does_not_advertise_unavailable_engines() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = ServerState::new(temp.path().to_path_buf());
    state.engine_allowlist = vec!["__missing_engine_for_test__".to_string()];

    assert!(state.advertised_engines().is_empty());
}

#[test]
fn test_engine_detection() {
    use prod_code_protocol::messages::EngineKind;

    let temp = tempfile::tempdir().unwrap();
    assert_eq!(detect_engine(temp.path()), EngineKind::Generic);

    std::fs::write(temp.path().join("Cargo.toml"), "").unwrap();
    assert_eq!(detect_engine(temp.path()), EngineKind::Rust);

    let go_temp = tempfile::tempdir().unwrap();
    std::fs::write(go_temp.path().join("go.mod"), "").unwrap();
    assert_eq!(detect_engine(go_temp.path()), EngineKind::Go);

    let py_temp = tempfile::tempdir().unwrap();
    std::fs::write(py_temp.path().join("pyproject.toml"), "").unwrap();
    assert_eq!(detect_engine(py_temp.path()), EngineKind::Python);

    let ts_temp = tempfile::tempdir().unwrap();
    std::fs::write(ts_temp.path().join("package.json"), "").unwrap();
    assert_eq!(detect_engine(ts_temp.path()), EngineKind::TypeScript);

    let java_temp = tempfile::tempdir().unwrap();
    std::fs::write(java_temp.path().join("pom.xml"), "").unwrap();
    assert_eq!(detect_engine(java_temp.path()), EngineKind::Java);

    let kt_temp = tempfile::tempdir().unwrap();
    std::fs::write(
        kt_temp.path().join("build.gradle.kts"),
        "plugins { kotlin(\"jvm\") }",
    )
    .unwrap();
    assert_eq!(detect_engine(kt_temp.path()), EngineKind::Kotlin);

    let cs_temp = tempfile::tempdir().unwrap();
    std::fs::write(cs_temp.path().join("App.csproj"), "").unwrap();
    assert_eq!(detect_engine(cs_temp.path()), EngineKind::Csharp);

    let php_temp = tempfile::tempdir().unwrap();
    std::fs::write(php_temp.path().join("composer.json"), "").unwrap();
    assert_eq!(detect_engine(php_temp.path()), EngineKind::Php);

    let rb_temp = tempfile::tempdir().unwrap();
    std::fs::write(rb_temp.path().join("Gemfile"), "").unwrap();
    assert_eq!(detect_engine(rb_temp.path()), EngineKind::Ruby);
}

#[tokio::test]
async fn test_apply_sync_create_and_delete() {
    let storage_temp = tempfile::tempdir().unwrap();
    let client_root = "/Users/testuser/Projects/my-app";

    let req = SyncRequest {
        client_workspace_root: client_root.to_string(),
        files: vec![
            FileDelta {
                relative_path: "src/lib.rs".to_string(),
                content: Some(b"pub fn add(a: i32, b: i32) -> i32 { a + b }".to_vec()),
                is_executable: false,
            },
            FileDelta {
                relative_path: "README.md".to_string(),
                content: Some(b"# My App".to_vec()),
                is_executable: false,
            },
        ],
        clean_others: false,
        base_workspace_name: None,
    };

    let resp = apply_sync(storage_temp.path(), &WorkspaceManager::new(), req).await;
    assert_eq!(resp.files_updated, 2);
    assert_eq!(resp.files_deleted, 0);

    let app_dir = storage_temp.path().join("my-app");
    assert!(app_dir.join("src/lib.rs").exists());
    assert!(app_dir.join("README.md").exists());
    let content = std::fs::read_to_string(app_dir.join("src/lib.rs")).unwrap();
    assert!(content.contains("pub fn add"));

    // Now test deleting README.md
    let del_req = SyncRequest {
        client_workspace_root: client_root.to_string(),
        files: vec![FileDelta {
            relative_path: "README.md".to_string(),
            content: None,
            is_executable: false,
        }],
        clean_others: false,
        base_workspace_name: None,
    };

    let del_resp = apply_sync(storage_temp.path(), &WorkspaceManager::new(), del_req).await;
    assert_eq!(del_resp.files_updated, 0);
    assert_eq!(del_resp.files_deleted, 1);
    assert!(!app_dir.join("README.md").exists());
    assert!(app_dir.join("src/lib.rs").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn sync_rejects_absolute_parent_and_symlink_paths() {
    use std::os::unix::fs::symlink;

    let storage = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let root = storage.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    symlink(outside.path(), root.join("linked")).unwrap();

    let victim = outside.path().join("victim.txt");
    std::fs::write(&victim, b"keep").unwrap();
    let absolute_write = outside.path().join("absolute-escape.txt");
    let parent_write = outside.path().join("parent-escape.txt");
    let symlink_write = outside.path().join("symlink-escape.txt");
    let response = apply_sync(
        storage.path(),
        &manager,
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
    )
    .await;

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

/// Both halves of the engine cache, in one test because the cache is process-global and
/// two tests would race for it. The sentinel is a value the probe cannot produce, so a
/// sentinel coming back proves the probe did not run, and a sentinel gone proves it did.
#[test]
fn engines_are_served_from_the_cache_until_it_expires() {
    let sentinel = vec!["sentinel (not a real engine)".to_string()];

    store_engines(Instant::now() + ENGINE_CACHE_TTL, sentinel.clone());
    assert_eq!(
        cached_available_engines(),
        sentinel,
        "a live entry must be answered without probing"
    );

    store_engines(Instant::now(), sentinel.clone());
    let fresh = cached_available_engines();
    assert_ne!(fresh, sentinel, "an expired entry must be probed again");
    assert!(
        fresh.iter().any(|e| e.starts_with("rust ")),
        "the probe always reports the in-process Rust engine, got {fresh:?}"
    );
    assert_eq!(
        cached_available_engines(),
        fresh,
        "the probe's answer is what the next caller gets"
    );
}
