/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::scan::*;
use crate::sync::*;

#[test]
fn test_scan_workspace_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::write(root.join("target/debug/app"), "binary").unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/config"), "git").unwrap();

    let deltas = scan_workspace_files(root, None).unwrap();
    let paths: Vec<_> = deltas.iter().map(|d| d.relative_path.as_str()).collect();

    assert!(paths.contains(&"src/main.rs") || paths.contains(&"src\\main.rs"));
    assert!(paths.contains(&"Cargo.toml"));
    assert!(!paths.iter().any(|p| p.contains("target")));
    assert!(!paths.iter().any(|p| p.contains(".git")));
}

#[test]
fn test_collect_dirty_files_in_git_repo() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // Initialize a git repo in tempdir
    let init_status = std::process::Command::new("git")
        .arg("init")
        .current_dir(root)
        .status();
    if init_status.is_err() || !init_status.unwrap().success() {
        return; // git not available in environment, skip
    }

    // Configure git user for commit
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(root)
        .status();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(root)
        .status();

    // 1. Initial committed file
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn original() {}").unwrap();
    let _ = std::process::Command::new("git")
        .args(["add", "src/lib.rs"])
        .current_dir(root)
        .status();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(root)
        .status();

    // 2. Modify existing file
    std::fs::write(root.join("src/lib.rs"), "pub fn modified() {}").unwrap();

    // 3. Create a brand new untracked file (NO git add)
    std::fs::write(root.join("src/untracked.rs"), "pub fn untracked() {}").unwrap();

    // 4. Create non-code / research files that MUST be ignored
    std::fs::create_dir_all(root.join("research")).unwrap();
    std::fs::write(root.join("research/bench.jsonl"), "{\"dump\": true}").unwrap();
    std::fs::write(root.join("notes.md"), "# Research Notes").unwrap();

    // 5. First collection: code files collected, non-code files ignored
    let deltas = collect_dirty_files_incremental(root).unwrap();
    let map: std::collections::HashMap<_, _> = deltas
        .into_iter()
        .map(|d| (d.relative_path, d.content))
        .collect();

    assert!(map.contains_key("src/lib.rs") || map.contains_key("src\\lib.rs"));
    assert!(map.contains_key("src/untracked.rs") || map.contains_key("src\\untracked.rs"));
    assert!(!map.contains_key("research/bench.jsonl"));
    assert!(!map.contains_key("notes.md"));

    let untracked_content = map
        .get("src/untracked.rs")
        .or_else(|| map.get("src\\untracked.rs"))
        .unwrap()
        .as_ref()
        .unwrap();
    assert_eq!(
        std::str::from_utf8(untracked_content).unwrap(),
        "pub fn untracked() {}"
    );

    // 6. Second consecutive collection without changes: cache hit, zero deltas!
    let deltas_cached = collect_dirty_files_incremental(root).unwrap();
    assert!(
        deltas_cached.is_empty(),
        "Expected 0 deltas on cache hit, got {}",
        deltas_cached.len()
    );

    // 7. Touch one file: only that file is collected again
    std::fs::write(root.join("src/lib.rs"), "pub fn modified_v2() {}").unwrap();
    let deltas_recheck = collect_dirty_files_incremental(root).unwrap();
    assert_eq!(deltas_recheck.len(), 1);
    assert!(deltas_recheck[0].relative_path.contains("lib.rs"));
}

#[test]

fn test_first_sync_includes_modified_tracked_file() {
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
    std::fs::write(root.join("src/a.rs"), "pub fn a() -> u8 { 1 }").unwrap();
    std::fs::write(root.join("src/b.rs"), "pub fn b() -> u8 { 1 }").unwrap();
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
    // Modified before any sync ever happened: must be sent with its modified content.
    std::fs::write(
        root.join("src/a.rs"),
        "pub fn a(divergent_marker: i64) -> u8 { 1 }",
    )
    .unwrap();
    let first = prepare_workspace_sync(root, None).unwrap();
    let mut names: Vec<&str> = first
        .files
        .iter()
        .map(|f| f.relative_path.as_str())
        .collect();
    names.sort();
    assert_eq!(names, vec!["src/a.rs", "src/b.rs"], "{first:?}");
    let a = first
        .files
        .iter()
        .find(|f| f.relative_path == "src/a.rs")
        .unwrap();
    assert!(
        std::str::from_utf8(a.content.as_deref().unwrap())
            .unwrap()
            .contains("divergent_marker")
    );
    clear_sync_cache(root);
}

#[test]
fn test_reverted_dirty_file_is_resent_clean() {
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
    std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 1 }").unwrap();
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
    let initial = prepare_workspace_sync(root, None).unwrap();
    commit_workspace_sync(root, &initial);

    // Dirty edit is sent and remembered as dirty.
    std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 2 }").unwrap();
    let dirty = prepare_workspace_sync(root, None).unwrap();
    assert_eq!(dirty.files.len(), 1);
    commit_workspace_sync(root, &dirty);
    assert!(
        load_sync_cache(&std::fs::canonicalize(root).unwrap())
            .dirty_paths
            .contains("src/lib.rs")
    );

    // Revert to HEAD: git reports nothing, yet the clean content must go out again.
    std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 1 }").unwrap();
    let reverted = prepare_workspace_sync(root, None).unwrap();
    assert_eq!(reverted.files.len(), 1);
    assert_eq!(
        reverted.files[0].content.as_deref(),
        Some("pub fn version() -> u8 { 1 }".as_bytes())
    );
    commit_workspace_sync(root, &reverted);
    assert!(prepare_workspace_sync(root, None).unwrap().files.is_empty());
    clear_sync_cache(root);
}
