/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Tests for engine lifecycle, direct mutation, priming, and snapshot concurrency.

use super::create_test_fixture;
use crate::engine::RustEngine;

#[test]
fn a_priming_job_infers_the_functions_of_its_files() {
    let (temp, lib_path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
    engine.set_label("fixture-engine");
    let unknown = temp.path().join("src/nowhere.rs");
    let job = engine.priming_job(&[lib_path.as_path(), unknown.as_path()]);
    // The job names the engine and revision it was made at, for its log (#1499).
    assert_eq!(job.identity(), ("fixture-engine", engine.changes));
    assert_eq!(job.threads(), 1, "one function, one thread");
    assert_eq!(job.files.len(), 1, "the unknown file is left out");
    assert_eq!(job.run(), 1);
    assert_eq!(engine.priming_job(&[unknown.as_path()]).threads(), 0);
    // The diagnostics that follow find the work done.
    assert!(engine.diagnostics(&lib_path).is_ok());
}

/// A file that is not Rust is not outlined as Rust, whether it was loaded from disk or opened
/// by a client (#247).
#[test]
fn a_file_that_is_not_rust_is_not_parsed_as_rust() {
    let (temp, _lib_path) = create_test_fixture();
    let readme = temp.path().join("README.md");
    std::fs::write(&readme, "Pick an interface or a class and go.\n").unwrap();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let manifest = temp.path().join("Cargo.toml");
    assert!(engine.file_id_for_path(&manifest).is_some());
    let manifest_error = engine.document_symbols(&manifest).unwrap_err();
    assert!(
        manifest_error.to_string().contains("is not a Rust file"),
        "{manifest_error}"
    );
    let existing_readme_error = engine.document_symbols(&readme).unwrap_err();
    assert!(
        existing_readme_error
            .to_string()
            .contains("is not a Rust file"),
        "{existing_readme_error}"
    );
    engine
        .apply_file_change(
            &readme,
            "Pick an interface or a class and go.\n".to_string(),
        )
        .unwrap();
    let err = engine.document_symbols(&readme).unwrap_err();
    assert!(format!("{err}").contains("is not a Rust file"), "{err}");
}

#[test]
fn manifest_declared_extensionless_rust_target_is_outlined() {
    let temp = tempfile::tempdir().unwrap();
    super::configure_test_workspace(temp.path());
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"extensionless_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib\"\n",
    )
    .unwrap();
    let src = temp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    let target = src.join("lib");
    std::fs::write(&target, "pub struct Extensionless;\npub fn visible() {}\n").unwrap();
    let unrelated = src.join("notes");
    std::fs::write(&unrelated, "ordinary text, not a Cargo target\n").unwrap();

    let metadata = std::process::Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(temp.path().join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        metadata.status.success(),
        "the fixture manifest should be valid: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
    assert!(
        metadata["packages"][0]["targets"][0]["src_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("src/lib")),
        "Cargo should expose the extensionless lib target: {metadata}"
    );

    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
    assert!(
        engine.file_id_for_path(&target).is_some(),
        "the manifest-referenced extensionless module should be loaded during workspace load"
    );
    let symbols = engine
        .document_symbols(&target)
        .expect("the manifest-declared Rust target has an outline");
    assert!(symbols.iter().any(|symbol| symbol.name == "visible"));
    std::fs::write(
        &target,
        "pub struct Extensionless;\npub fn visible() {}\npub fn after_reload() {}\n",
    )
    .unwrap();
    engine
        .reload_base()
        .expect("manifest reload keeps the extensionless target in the crate graph");
    let reloaded_symbols = engine
        .document_symbols(&target)
        .expect("the reloaded Cargo target still has an outline");
    assert!(
        reloaded_symbols
            .iter()
            .any(|symbol| symbol.name == "after_reload")
    );
    engine
        .reload_file(&unrelated)
        .expect("the unrelated file can be observed in the VFS");
    assert!(
        engine.file_id_for_path(&unrelated).is_some(),
        "an unrelated file can exist in the VFS without being a Rust crate member"
    );
    let error = engine.document_symbols(&unrelated).unwrap_err();
    assert!(error.to_string().contains("is not a Rust file"), "{error}");
}

#[test]
fn feature_gated_path_outside_package_is_loaded_and_resolved() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let consumer_src = root.join("consumer/src");
    std::fs::create_dir_all(&consumer_src).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"consumer\"]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("prod-code.toml"),
        "[rust]\nfeatures = \"all\"\nproc_macro_srv = \"disabled\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("consumer/Cargo.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ndefault = [\"shared\"]\nshared = []\n",
    )
    .unwrap();
    let consumer_lib = consumer_src.join("lib.rs");
    std::fs::write(
        &consumer_lib,
        "#[cfg(feature = \"shared\")]\n#[path = \"../../shared.rs\"]\nmod shared;\npub fn call_shared() { shared::external_shared_function(); }\n",
    )
    .unwrap();
    let shared = root.join("shared.rs");
    std::fs::write(&shared, "pub fn external_shared_function() {}\n").unwrap();

    let mut engine = RustEngine::load(root).expect("load the workspace with all features");
    assert!(
        engine.file_id_for_path(&shared).is_some(),
        "the feature-gated external path module should be loaded during workspace load"
    );
    let symbols = engine
        .document_symbols(&shared)
        .expect("the external path module should have document symbols");
    assert!(
        symbols
            .iter()
            .any(|symbol| symbol.name == "external_shared_function"),
        "{symbols:?}"
    );
    let workspace_symbols = engine
        .workspace_symbols("external_shared_function", 10)
        .expect("search the loaded workspace for the external declaration");
    assert!(
        workspace_symbols
            .iter()
            .any(|symbol| symbol.name == "external_shared_function"),
        "expected the external declaration in workspace symbol search, got {workspace_symbols:?}"
    );
    let definitions = engine
        .goto_definition(&consumer_lib, 4, 32)
        .expect("resolve the external module from its consumer");
    assert!(
        definitions
            .iter()
            .any(|target| { target.path == shared && target.name == "external_shared_function" }),
        "expected the consumer call to resolve into the shared file, got {definitions:?}"
    );
    engine
        .reload_base()
        .expect("manifest reload preserves the external path module");
    let reloaded_definitions = engine
        .goto_definition(&consumer_lib, 4, 32)
        .expect("resolve the external module after manifest reload");
    assert!(
        reloaded_definitions
            .iter()
            .any(|target| { target.path == shared && target.name == "external_shared_function" }),
        "expected reload to keep the external definition, got {reloaded_definitions:?}"
    );
}

#[test]
fn test_rust_engine_direct_mutation() {
    let (temp, lib_path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");

    // Apply in-memory direct edit adding a new constant
    let new_code = "pub const IN_MEMORY_SUPER_FAST: u32 = 42;\n".to_string();
    engine
        .apply_file_change(&lib_path, new_code)
        .expect("Must apply direct change to Salsa DB");

    // Verify that in-memory symbols immediately reflect the new symbol without saving to disk!
    let syms = engine
        .document_symbols(&lib_path)
        .expect("Must get updated symbols");
    assert!(syms.iter().any(|s| s.name == "IN_MEMORY_SUPER_FAST"));

    // Verify hover on the new in-memory symbol!
    let hover = engine
        .hover(&lib_path, 1, 15)
        .expect("Must query hover on newly added in-memory symbol");
    assert!(hover.is_some());
    assert!(hover.unwrap().contains("IN_MEMORY_SUPER_FAST"));
}

#[test]
fn test_identical_text_does_not_change_revision() {
    let (temp, lib_path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let text = std::fs::read_to_string(&lib_path).unwrap();
    engine.apply_file_change(&lib_path, text.clone()).unwrap();
    // A snapshot taken now survives a no-op re-open; a real revision would cancel it.
    let snapshot = engine.snapshot();
    engine.apply_file_change(&lib_path, text.clone()).unwrap();
    assert!(snapshot.hover(&lib_path, 1, 15).unwrap().is_some());
    // Release the snapshot before a real change: apply_change waits for outstanding
    // snapshots, so holding one here would deadlock the test.
    drop(snapshot);
    // A genuine change still lands.
    engine
        .apply_file_change(&lib_path, format!("{text}pub const CHANGED: u8 = 1;\n"))
        .unwrap();
    assert!(
        engine
            .document_symbols(&lib_path)
            .unwrap()
            .iter()
            .any(|s| s.name == "CHANGED")
    );
}

#[test]
fn test_rust_engine_add_new_untracked_file() {
    let (temp, lib_path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");

    // Dynamically add a brand new untracked file that was never loaded initially
    let new_file_path = temp.path().join("src/helper.rs");
    let helper_code = "pub fn dynamic_helper() -> u32 { 1337 }\n".to_string();
    engine
        .apply_file_change(&new_file_path, helper_code)
        .expect("Must apply change for brand new file without panic");

    let syms = engine
        .document_symbols(&new_file_path)
        .expect("Must get document symbols for newly added file");
    assert!(syms.iter().any(|s| s.name == "dynamic_helper"));

    // Link helper module into lib.rs so Salsa builds the module tree and HIR
    let mut lib_code = std::fs::read_to_string(&lib_path).unwrap();
    lib_code.push_str("\npub mod helper;\n");
    engine
        .apply_file_change(&lib_path, lib_code)
        .expect("Must update lib.rs");

    let hover = engine
        .hover(&new_file_path, 1, 10)
        .expect("Must query hover on newly added file");
    assert!(hover.is_some());
    assert!(hover.unwrap().contains("fn dynamic_helper"));
}

#[test]
fn test_snapshot_parallel_execution() {
    let (temp, lib_path) = create_test_fixture();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");

    let snap1 = engine.snapshot();
    let snap2 = engine.snapshot();
    let p1 = lib_path.clone();
    let p2 = lib_path;

    let h1 = std::thread::spawn(move || snap1.hover(&p1, 1, 15));
    let h2 = std::thread::spawn(move || snap2.document_symbols(&p2));

    let res1 = h1.join().unwrap().unwrap();
    let res2 = h2.join().unwrap().unwrap();

    assert!(res1.is_some());
    assert!(!res2.is_empty());
}

#[test]
fn test_rust_engine_proc_macro_farm_lifecycle() {
    let (temp, lib_path) = create_test_fixture();

    // Write prod-code.toml specifying sandboxed worker with 2 workers
    std::fs::write(
        temp.path().join("prod-code.toml"),
        "[rust]\nbuild_scripts = true\nproc_macro_srv = \"sandboxed\"\nproc_macro_workers = 2\n",
    )
    .unwrap();

    let engine =
        RustEngine::load(temp.path()).expect("Must load fixture with sandboxed proc-macro farm");

    let active_metrics = RustEngine::proc_macro_farm_metrics();
    assert!(
        active_metrics.active_workers >= 1,
        "Must have active workers while engine is alive"
    );
    assert!(
        active_metrics.active_workspaces >= 1,
        "Must have active workspaces while engine is alive"
    );

    // Document symbols on fixture continue to work
    let syms = engine.document_symbols(&lib_path).unwrap();
    assert!(syms.iter().any(|s| s.name == "DEFAULT_PORT"));

    // Dropping engine releases worker permits back to shared farm
    drop(engine);
}
