/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::engine::RustEngine;
use crate::tests::configure_test_workspace;

const MANIFEST: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str = "pub fn rename_me() -> u32 {\n    42\n}\n";

fn write_crate(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    configure_test_workspace(root);
    std::fs::write(root.join("Cargo.toml"), MANIFEST).unwrap();
    std::fs::write(root.join("src/lib.rs"), LIB).unwrap();
}

/// A base workspace and a second crate directory to attach as a worktree copy.
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, RustEngine) {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base");
    let copy = dir.path().join("copy");
    write_crate(&base);
    write_crate(&copy);
    let engine = RustEngine::load(&base).expect("base loads");
    (dir, base, copy, engine)
}

fn touch(path: &Path) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(3600))
        .unwrap();
}

#[test]
fn base_manifest_paths_are_sorted_deduplicated_and_include_config_files() {
    let (_d, base, _copy, engine) = fixture();
    assert_eq!(
        engine.collect_base_manifest_paths(),
        [
            ".cargo/config",
            ".cargo/config.toml",
            "Cargo.lock",
            "Cargo.toml",
            "rust-toolchain",
            "rust-toolchain.toml"
        ]
        .map(|p| base.join(p))
    );
}

#[test]
fn worktree_manifest_paths_follow_the_copy_root() {
    let (_d, _base, copy, engine) = fixture();
    let want = [
        ".cargo/config",
        ".cargo/config.toml",
        "Cargo.lock",
        "Cargo.toml",
        "rust-toolchain",
        "rust-toolchain.toml",
    ]
    .map(|p| copy.join(p));
    // Without a loaded workspace the base's manifests are mapped under the copy root.
    assert_eq!(engine.collect_worktree_manifest_paths(&copy, None), want);
}

#[test]
fn recorded_mtimes_cover_existing_and_missing_files() {
    let (_d, base, copy, mut engine) = fixture();
    engine.record_base_manifests();
    let recorded = &engine.base_manifest_mtimes;
    assert!(recorded[&base.join("Cargo.toml")].is_some());
    assert_eq!(recorded[&base.join(".cargo/config")], None);
    assert_eq!(recorded[&base.join("rust-toolchain.toml")], None);
    engine.record_worktree_manifests(&copy, None);
    let wt = &engine.worktree_manifest_mtimes[&copy];
    assert!(wt[&copy.join("Cargo.toml")].is_some());
    assert_eq!(wt[&copy.join(".cargo/config.toml")], None);
}

#[test]
fn base_staleness_follows_manifest_mtimes_and_reload_clears_it() {
    let (_d, base, _copy, mut engine) = fixture();
    assert!(!engine.is_base_stale());
    touch(&base.join("Cargo.toml"));
    assert!(engine.is_base_stale());
    let before = engine.changes;
    engine.reload_base().expect("reload");
    assert!(!engine.is_base_stale());
    assert_eq!(engine.changes, before + 1);
    // A manifest that appears counts as a change too.
    std::fs::write(base.join("rust-toolchain"), "").unwrap();
    assert!(engine.is_base_stale());
    std::fs::remove_file(base.join("rust-toolchain")).unwrap();
    assert!(!engine.is_base_stale());
    // Nothing recorded: never stale.
    touch(&base.join("Cargo.toml"));
    engine.base_manifest_mtimes.clear();
    assert!(!engine.is_base_stale());
}

#[test]
fn worktree_staleness_covers_unknown_base_and_own_manifests() {
    let (_d, base, copy, mut engine) = fixture();
    assert!(!engine.is_worktree_stale(&copy), "not attached");
    engine.attach_worktree(&copy).expect("attach");
    assert!(engine.has_worktree(&copy));
    assert!(!engine.is_worktree_stale(&copy));
    assert!(
        !engine.is_worktree_stale(&base),
        "the base is not a worktree"
    );
    touch(&copy.join("Cargo.toml"));
    assert!(engine.is_worktree_stale(&copy));
    engine.reload_worktree(&copy).expect("reload copy");
    assert!(!engine.is_worktree_stale(&copy));
    // A stale base makes every worktree stale.
    touch(&base.join("Cargo.toml"));
    assert!(engine.is_worktree_stale(&copy));
    // An attached worktree without recorded mtimes is stale.
    engine.reload_base().unwrap();
    engine.worktree_manifest_mtimes.remove(&copy);
    assert!(engine.is_worktree_stale(&copy));
}

#[test]
fn ensure_fresh_reloads_what_is_stale_and_reports_it() {
    let (_d, base, copy, mut engine) = fixture();
    engine.attach_worktree(&copy).expect("attach");
    let base_lib = base.join("src/lib.rs");
    let copy_lib = copy.join("src/lib.rs");
    assert!(!engine.ensure_fresh_for_path(&base_lib).unwrap());
    assert!(!engine.ensure_fresh_for_path(&copy_lib).unwrap());
    assert!(
        !engine
            .ensure_fresh_for_path(Path::new("/nowhere/else.rs"))
            .unwrap()
    );
    touch(&base.join("Cargo.toml"));
    assert!(engine.ensure_fresh_for_path(&base_lib).unwrap());
    assert!(!engine.is_base_stale());
    touch(&copy.join("Cargo.toml"));
    assert!(engine.ensure_fresh_for_path(&copy_lib).unwrap());
    assert!(!engine.is_worktree_stale(&copy));
}

#[test]
fn reload_failures_name_the_missing_manifest() {
    let (dir, base, _copy, mut engine) = fixture();
    let missing = dir.path().join("absent");
    let err = engine.reload_worktree(&missing).unwrap_err().to_string();
    assert!(
        err.starts_with(&format!(
            "Manifest discovery failed for copy {}",
            missing.display()
        )),
        "{err}"
    );
    std::fs::remove_file(base.join("Cargo.toml")).unwrap();
    let err = engine.reload_base().unwrap_err().to_string();
    assert!(err.starts_with("Manifest discovery failed:"), "{err}");
}

#[test]
fn new_integration_test_target_in_worktree_triggers_reload_and_indexes_symbols() {
    let (_d, _base, copy, mut engine) = fixture();
    engine.attach_worktree(&copy).expect("attach");
    assert!(!engine.is_worktree_stale(&copy));

    let tests_dir = copy.join("tests");
    std::fs::create_dir_all(&tests_dir).unwrap();
    let new_test = tests_dir.join("lsp_reconnect.rs");
    std::fs::write(&new_test, "pub fn integration_only_helper() -> u32 { 7 }\n").unwrap();

    assert!(
        engine.is_worktree_stale(&copy),
        "new integration test target in worktree must mark worktree stale even when Cargo.toml is unchanged"
    );
    assert!(engine.ensure_fresh_for_path(&new_test).unwrap());
    assert!(!engine.is_worktree_stale(&copy));

    let syms = engine
        .workspace_symbols_for(&copy, "integration_only_helper", 32)
        .unwrap();
    assert!(
        syms.iter().any(|s| s.name == "integration_only_helper"),
        "reloaded worktree must index integration test symbols: {syms:?}"
    );
}
