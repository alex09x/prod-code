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
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let unknown = temp.path().join("src/nowhere.rs");
    let job = engine.priming_job(&[lib_path.as_path(), unknown.as_path()]);
    assert_eq!(job.threads(), 1, "one function, one thread");
    assert_eq!(job.files.len(), 1, "the unknown file is left out");
    assert_eq!(job.run(), 1);
    assert_eq!(engine.priming_job(&[unknown.as_path()]).threads(), 0);
    // The diagnostics that follow find the work done.
    assert!(engine.diagnostics(&lib_path).is_ok());
}

/// A file that is not Rust is neither added to the database when a client opens it nor
/// outlined as if it were (#247).
#[test]
fn a_file_that_is_not_rust_is_not_parsed_as_rust() {
    let (temp, _lib_path) = create_test_fixture();
    let mut engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let readme = temp.path().join("README.md");
    std::fs::write(&readme, "Pick an interface or a class and go.\n").unwrap();
    engine
        .apply_file_change(
            &readme,
            "Pick an interface or a class and go.\n".to_string(),
        )
        .unwrap();
    assert!(
        engine.file_id_for_path(&readme).is_none(),
        "the README stays out"
    );
    let err = engine.document_symbols(&readme).unwrap_err();
    assert!(format!("{err}").contains("is not a Rust file"), "{err}");
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
