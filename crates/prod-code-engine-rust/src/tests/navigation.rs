/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Tests for in-memory navigation, symbols, hover, and position bounds.

use ra_ap_ide::TextSize;

use super::create_test_fixture;
use crate::engine::RustEngine;
use crate::vfs::offset_to_line_col;

#[test]
fn test_rust_engine_in_memory_queries() {
    let (temp, lib_path) = create_test_fixture();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture directly into Salsa DB");

    // Test document symbols on fixture lib.rs
    let syms = engine
        .document_symbols(&lib_path)
        .expect("Must get document symbols");
    assert!(!syms.is_empty(), "Must find symbols in fixture lib.rs");
    assert!(syms.iter().any(|s| s.name == "DEFAULT_PORT"));
    assert!(syms.iter().any(|s| s.name == "PathTranslator"));

    // Test in-memory hover on DEFAULT_PORT (line 1, col 15)
    let hover = engine
        .hover(&lib_path, 1, 15)
        .expect("Must query in-memory hover");
    assert!(hover.is_some(), "Hover must resolve for DEFAULT_PORT");
    assert!(hover.unwrap().contains("DEFAULT_PORT"));

    // Test in-memory jump to definition for PathTranslator (line 3, col 15)
    let defs = engine
        .goto_definition(&lib_path, 3, 15)
        .expect("Must query definition");
    assert!(
        !defs.is_empty(),
        "Must resolve definition for PathTranslator"
    );
    assert!(defs.iter().any(|d| d.name == "PathTranslator"));
}

#[test]
fn invalid_positions_fail_before_native_queries_or_refactors() {
    let (temp, lib_path) = create_test_fixture();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let assert_invalid = |err: anyhow::Error| {
        assert!(
            format!("{err:#}").contains("Invalid position 999999:1"),
            "{err:#}"
        );
    };

    for (line, col) in [(0, 1), (1, 0), (1, 999999)] {
        let error = engine.hover(&lib_path, line, col).unwrap_err();
        assert!(
            format!("{error:#}").contains("Invalid position"),
            "{error:#}"
        );
    }
    assert_invalid(engine.hover(&lib_path, 999999, 1).unwrap_err());
    assert_invalid(engine.rename(&lib_path, 999999, 1, "Renamed").unwrap_err());
    assert_invalid(engine.goto_definition(&lib_path, 999999, 1).unwrap_err());
    assert_invalid(engine.find_all_refs(&lib_path, 999999, 1).unwrap_err());
    assert_invalid(
        engine
            .prepare_call_hierarchy(&lib_path, 999999, 1)
            .unwrap_err(),
    );
    assert_invalid(engine.incoming_calls(&lib_path, 999999, 1).unwrap_err());
    assert_invalid(engine.outgoing_calls(&lib_path, 999999, 1).unwrap_err());
    assert_invalid(
        engine
            .goto_implementation(&lib_path, 999999, 1)
            .unwrap_err(),
    );
    assert_invalid(engine.safe_delete(&lib_path, 999999, 1).unwrap_err());
    assert_invalid(
        engine
            .list_assists(&lib_path, 1, 15, Some((999999, 1)))
            .unwrap_err(),
    );
    assert_invalid(
        engine
            .apply_assist(&lib_path, 1, 15, Some((999999, 1)), "x", None)
            .unwrap_err(),
    );
    assert_invalid(
        engine
            .structural_replace("DEFAULT_PORT ==>> DEFAULT_PORT", &lib_path, 999999, 1, None)
            .unwrap_err(),
    );
}

#[test]
fn native_queries_accept_utf16_and_eof_positions() {
    let (temp, lib_path) = create_test_fixture();
    let text = "/// 😀\r\npub const DEFAULT_PORT: u16 = 9400;\r\npub fn query() -> u16 { let _ = \"😀\"; DEFAULT_PORT }\r\n";
    std::fs::write(&lib_path, text).unwrap();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let target = text.rfind("DEFAULT_PORT").unwrap();
    let line_start = text[..target].rfind('\n').map_or(0, |at| at + 1);
    let col = text[line_start..target].encode_utf16().count() as u32 + 1;
    let hover = engine
        .hover(&lib_path, 3, col)
        .expect("valid UTF-16 position");
    assert!(hover.is_some(), "the symbol after 😀 resolves");

    let snapshot = engine.snapshot();
    let forward = snapshot.file_range(&lib_path, 2, 1, Some((2, 4))).unwrap();
    let reversed = snapshot.file_range(&lib_path, 2, 4, Some((2, 1))).unwrap();
    assert_eq!(
        forward.range, reversed.range,
        "valid reversed selections stay accepted"
    );
    assert!(
        snapshot
            .file_range(&lib_path, 2, 1, None)
            .unwrap()
            .range
            .is_empty()
    );
    let eof = TextSize::of(text);
    let (line, col) = offset_to_line_col(text, eof);
    assert!(
        engine.hover(&lib_path, line, col).is_ok(),
        "valid EOF position"
    );
    assert!(
        engine.hover(&lib_path, 1, 6).is_err(),
        "a UTF-16 surrogate interior is rejected"
    );
    assert!(
        engine.hover(&lib_path, 1, 8).is_err(),
        "a CRLF-adjacent oversized column is rejected"
    );
}

/// The symbol with the very name is found however many names that only hold its letters
/// sort before it (#348): `a_run_0` to `a_run_29` fill a limit of 10 on their own.
#[test]
fn an_exact_name_is_found_past_the_fuzzy_matches_that_fill_the_limit() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let mut lib = String::from("pub mod proxy;\n");
    for i in 0..30 {
        lib.push_str(&format!("pub fn a_run_{i}() {{}}\n"));
    }
    std::fs::write(ws.join("src/lib.rs"), lib).unwrap();
    std::fs::write(ws.join("src/proxy.rs"), "pub fn run() {}\n").unwrap();
    let engine = RustEngine::load(ws).expect("Must load fixture");

    let found = engine.workspace_symbols("run", 10).unwrap();
    let first = found.first().unwrap_or_else(|| panic!("no hits"));
    assert_eq!(first.name, "run", "{found:?}");
    assert!(first.path.ends_with("src/proxy.rs"), "{first:?}");
    assert_eq!(
        found.len(),
        10,
        "the fuzzy matches fill the rest: {found:?}"
    );
}

/// A name the workspace's own crates lack is looked up in the dependency crates, and the
/// hit names its crate; a workspace hit keeps the dependencies out (#246). The dependency
/// is a vendored registry crate: rust-analyzer counts a path dependency as a workspace
/// crate, so only a registry-sourced one lands in the library roots.
#[test]
fn symbol_search_falls_back_to_dependency_crates() {
    let temp = tempfile::tempdir().unwrap();
    let vendor = temp.path().join("vendor");
    let dep = vendor.join("gadget");
    std::fs::create_dir_all(dep.join("src")).unwrap();
    std::fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname = \"gadget\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        dep.join(".cargo-checksum.json"),
        "{\"files\":{},\"package\":null}",
    )
    .unwrap();
    std::fs::write(
        dep.join("src/lib.rs"),
        "pub mod parts {\n    pub struct DepOnlyGadget;\n}\n\npub struct SharedName;\n\npub struct Analysis;\n\nimpl Analysis {\n    pub fn completions(&self) {}\n}\n",
    )
    .unwrap();

    let ws = temp.path().join("app");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::create_dir_all(ws.join(".cargo")).unwrap();
    std::fs::write(
        ws.join("Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ngadget = \"0.1\"\n",
    )
    .unwrap();
    std::fs::write(
        ws.join(".cargo/config.toml"),
        format!(
            "[source.crates-io]\nreplace-with = \"vendored\"\n\n[source.vendored]\ndirectory = \"{}\"\n",
            vendor.display()
        ),
    )
    .unwrap();
    std::fs::write(
        ws.join("src/lib.rs"),
        "pub struct SharedName;\n\npub struct RustAnalysisOptions;\n\npub fn make() -> gadget::parts::DepOnlyGadget {\n    gadget::parts::DepOnlyGadget\n}\n",
    )
    .unwrap();
    let engine = RustEngine::load(&ws).expect("Must load fixture");

    let found = engine.workspace_symbols("DepOnlyGadget", 10).unwrap();
    let hit = found
        .iter()
        .find(|s| s.name == "DepOnlyGadget")
        .unwrap_or_else(|| panic!("the dependency's struct is found: {found:?}"));
    assert!(
        hit.path.to_string_lossy().contains("/vendor/gadget/"),
        "{hit:?}"
    );
    assert_eq!(hit.container.as_deref(), Some("gadget::parts"), "{hit:?}");
    assert_eq!((hit.line, hit.col), (2, 16), "{hit:?}");

    // A workspace name that only resembles the query does not keep the dependency's own
    // symbol of that name out, and that one comes first (#328).
    let analysis = engine.workspace_symbols("Analysis", 10).unwrap();
    assert!(
        analysis.first().is_some_and(
            |s| s.name == "Analysis" && s.path.to_string_lossy().contains("/vendor/gadget/")
        ),
        "the dependency's `Analysis` first: {analysis:?}"
    );
    // A method's container is its type's path, so `Analysis::completions` resolves.
    let completions = engine.workspace_symbols("completions", 10).unwrap();
    assert!(
        completions
            .iter()
            .any(|s| s.name == "completions" && s.container.as_deref() == Some("gadget::Analysis")),
        "{completions:?}"
    );

    let shared = engine.workspace_symbols("SharedName", 10).unwrap();
    assert!(!shared.is_empty(), "the workspace struct is found");
    assert!(
        shared
            .iter()
            .all(|s| !s.path.to_string_lossy().contains("/vendor/") && s.container.is_none()),
        "a workspace hit suppresses the fallback: {shared:?}"
    );
}
