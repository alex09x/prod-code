/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::cache::*;
use crate::sync::plan::*;
use crate::sync::*;
use prod_code_protocol::content_hash;
use std::collections::HashSet;

#[test]
fn test_workspace_sync_state_uses_base_commit_and_watermarks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    clear_sync_cache(root);

    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    if !init.success() {
        return;
    }
    for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
        let configured = std::process::Command::new("git")
            .args(["config", key, value])
            .current_dir(root)
            .status()
            .unwrap();
        assert!(configured.success());
    }

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 1 }").unwrap();
    for args in [
        &["add", "src/lib.rs"][..],
        &["commit", "-qm", "initial"][..],
    ] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success());
    }

    let first = prepare_workspace_sync(root, None).unwrap();
    assert_eq!(first.files.len(), 1);
    commit_workspace_sync(root, &first);
    let state = load_sync_cache(root);
    assert!(state.base_commit_sha.is_some());
    assert!(state.last_sync_timestamp_ms > 0);
    assert!(
        state
            .files
            .get("src/lib.rs")
            .is_some_and(|entry| entry.hash != 0)
    );

    let unchanged = prepare_workspace_sync(root, None).unwrap();
    assert!(unchanged.files.is_empty());

    std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 2 }").unwrap();
    for args in [
        &["add", "src/lib.rs"][..],
        &["commit", "-qm", "changed"][..],
    ] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success());
    }

    let changed = prepare_workspace_sync(root, None).unwrap();
    assert_eq!(changed.files.len(), 1);
    assert_eq!(changed.files[0].relative_path, "src/lib.rs");
    clear_sync_cache(root);
}

#[test]
fn test_unreachable_base_commit_falls_back_to_full_tree() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    clear_sync_cache(root);

    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    if !init.success() {
        return;
    }
    for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
        let configured = std::process::Command::new("git")
            .args(["config", key, value])
            .current_dir(root)
            .status()
            .unwrap();
        assert!(configured.success());
    }

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 1 }").unwrap();
    for args in [&["add", "src"][..], &["commit", "-qm", "initial"][..]] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let initial = prepare_workspace_sync(root, None).unwrap();
    commit_workspace_sync(root, &initial);

    // Simulate history rewritten underneath the persisted watermark.
    let canonical = std::fs::canonicalize(root).unwrap();
    let mut state = load_sync_cache(&canonical);
    state.base_commit_sha = Some("0123456789abcdef0123456789abcdef01234567".to_string());
    save_sync_cache(&canonical, &state);

    // Unchanged file: fallback scans the full tree but the watermark still filters it.
    let unchanged = prepare_workspace_sync(root, None).unwrap();
    assert!(unchanged.files.is_empty());

    // Changed file: fallback still finds it.
    std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 3 }").unwrap();
    let changed = prepare_workspace_sync(root, None).unwrap();
    assert_eq!(changed.files.len(), 1);
    assert_eq!(changed.files[0].relative_path, "src/lib.rs");
    clear_sync_cache(root);
}

#[test]
fn test_initial_plan_carries_manifest_and_retains_only_missing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    clear_sync_cache(root);
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    if !init.success() {
        return;
    }
    for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
        assert!(
            std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "pub fn a() {}").unwrap();
    std::fs::write(root.join("src/b.rs"), "pub fn b() {}").unwrap();
    for args in [&["add", "src"][..], &["commit", "-qm", "initial"][..]] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
    }
    let mut plan = prepare_workspace_sync(root, None).unwrap();
    assert!(plan.initial);
    let manifest = plan.manifest();
    assert_eq!(manifest.len(), 2);
    assert_eq!(manifest[0].relative_path, "src/a.rs");
    assert_eq!(manifest[0].hash, content_hash(b"pub fn a() {}"));
    assert_eq!(manifest[0].size, 13);

    let keep: HashSet<String> = ["src/b.rs".to_string()].into_iter().collect();
    plan.retain_uploads(&keep);
    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].relative_path, "src/b.rs");
    commit_workspace_sync(root, &plan);

    // The skipped file counts as synced: the next plan is a pure delta and not initial.
    let next = prepare_workspace_sync(root, None).unwrap();
    assert!(!next.initial);
    assert!(next.files.is_empty());
    clear_sync_cache(root);
}

#[test]
fn test_stale_filter_version_forces_manifest_probe() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    clear_sync_cache(root);
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    if !init.success() {
        return;
    }
    for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
        assert!(
            std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(root.join("Cargo.lock"), "# lock").unwrap();
    std::fs::write(root.join("a.rs"), "pub fn a() {}").unwrap();
    for args in [&["add", "."][..], &["commit", "-qm", "initial"][..]] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
    }
    // A watermark written by an older client that never sent Cargo.lock.
    let first = prepare_workspace_sync(root, None).unwrap();
    commit_workspace_sync(root, &first);
    let canonical = std::fs::canonicalize(root).unwrap();
    let mut old = load_sync_cache(&canonical);
    old.filter_version = 0;
    old.files.remove("Cargo.lock");
    save_sync_cache(&canonical, &old);

    let upgraded = prepare_workspace_sync(root, None).unwrap();
    assert!(
        upgraded.initial,
        "old watermark must be treated as first contact"
    );
    assert!(
        upgraded
            .files
            .iter()
            .any(|f| f.relative_path == "Cargo.lock"),
        "{upgraded:?}"
    );
    commit_workspace_sync(root, &upgraded);
    assert_eq!(
        load_sync_cache(&canonical).filter_version,
        RELEVANCE_VERSION
    );
    assert!(!prepare_workspace_sync(root, None).unwrap().initial);
    clear_sync_cache(root);
}
