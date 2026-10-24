/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{ProcMacroCapacityError, RustEngine};
use crate::tests::configure_test_workspace;

#[test]
fn detects_rust_workspaces_by_their_manifest() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!RustEngine::is_rust_workspace(dir.path()));
    std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
    assert!(RustEngine::is_rust_workspace(dir.path()));
    assert!(!RustEngine::is_rust_workspace(&dir.path().join("nope")));
}

#[test]
fn capacity_error_reports_the_farm_capacity() {
    let err = ProcMacroCapacityError::new(8);
    assert_eq!(
        err.to_string(),
        "proc-macro worker farm had no capacity after waiting (capacity 8); retry the workspace load or set `proc_macro_srv = \"disabled\"` to opt out explicitly"
    );
}

#[test]
fn loading_a_directory_without_a_manifest_names_the_discovery_failure() {
    let dir = tempfile::tempdir().unwrap();
    let err = RustEngine::load(dir.path())
        .err()
        .expect("no manifest")
        .to_string();
    assert!(err.starts_with("Manifest discovery failed:"), "{err}");
}

#[test]
fn a_broken_manifest_fails_the_project_load() {
    let dir = tempfile::tempdir().unwrap();
    configure_test_workspace(dir.path());
    std::fs::write(dir.path().join("Cargo.toml"), "[package\nname = ").unwrap();
    let err = RustEngine::load(dir.path())
        .err()
        .expect("broken manifest")
        .to_string();
    assert!(err.starts_with("Project workspace load failed:"), "{err}");
}

fn small_crate(extra_config: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let lib = dir.path().join("src/lib.rs");
    std::fs::write(&lib, "pub fn rename_me() -> u32 {\n    42\n}\n").unwrap();
    std::fs::write(dir.path().join("prod-code.toml"), extra_config).unwrap();
    (dir, lib)
}

fn symbol_names(engine: &RustEngine, lib: &std::path::Path) -> Vec<String> {
    engine
        .document_symbols(lib)
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect()
}

#[test]
fn proc_macros_stay_off_when_build_scripts_are_off() {
    let (dir, lib) = small_crate("[rust]\nbuild_scripts = false\n");
    let engine = RustEngine::load(dir.path()).expect("loads without build scripts");
    assert_eq!(symbol_names(&engine, &lib), ["rename_me"]);
}

#[test]
fn the_sysroot_proc_macro_server_mode_loads_a_workspace() {
    let (dir, lib) = small_crate("[rust]\nproc_macro_srv = \"sysroot\"\n");
    let engine = RustEngine::load(dir.path()).expect("loads with the sysroot server");
    assert_eq!(symbol_names(&engine, &lib), ["rename_me"]);
}

#[test]
fn a_relative_workspace_root_is_resolved_against_the_current_directory() {
    let (dir, _lib) = small_crate("[rust]\nproc_macro_srv = \"disabled\"\n");
    let cwd = std::env::current_dir().unwrap();
    let mut relative = std::path::PathBuf::new();
    for _ in cwd.components().skip(1) {
        relative.push("..");
    }
    relative.push(dir.path().strip_prefix("/").unwrap());
    let engine = RustEngine::load(&relative).expect("loads through a relative path");
    assert_eq!(engine.workspace_root, relative);
    assert!(!engine.is_base_stale());
    assert!(
        engine
            .collect_base_manifest_paths()
            .contains(&dir.path().join("Cargo.toml"))
    );
}
