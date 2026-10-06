/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Tests for diagnostics reporting, unresolved paths, and import scoping.

use crate::engine::RustEngine;
use crate::vfs::imported_in_scope;

#[test]
fn test_a_use_in_an_enclosing_scope_counts_as_an_import() {
    use ra_ap_syntax::{AstNode, Edition, SourceFile, ast};
    let file = SourceFile::parse(
        concat!(
            "fn t() {\n",
            "    use std::sync::Mutex;\n",
            "    { let m: Mutex<u8> = todo!(); let h: HashMap<u8, u8> = todo!(); }\n",
            "}\n",
            "fn u() { let m: Mutex<u8> = todo!(); }\n",
            "mod m {\n",
            "    use opaque::Driver;\n",
            "    fn v() { let d: Driver = todo!(); }\n",
            "}\n",
            "fn w() { let d: Driver = todo!(); }\n",
        ),
        Edition::Edition2021,
    )
    .tree();
    let segment = |nth: usize, name: &str| {
        file.syntax()
            .descendants()
            .filter_map(ast::PathSegment::cast)
            .filter(|s| s.name_ref().is_some_and(|n| n.text() == name))
            .nth(nth)
            .unwrap()
    };
    // The `use` itself is the 0th `Mutex` segment; the annotation below it is the 1st.
    assert!(imported_in_scope(segment(1, "Mutex").syntax(), "Mutex"));
    assert!(!imported_in_scope(
        segment(0, "HashMap").syntax(),
        "HashMap"
    ));
    // Another function's `use` does not reach here.
    assert!(!imported_in_scope(segment(2, "Mutex").syntax(), "Mutex"));
    // A module's `use` reaches its functions, and not the functions outside it.
    assert!(imported_in_scope(segment(1, "Driver").syntax(), "Driver"));
    assert!(!imported_in_scope(segment(2, "Driver").syntax(), "Driver"));
}

#[test]
fn test_diagnostics_report_paths_that_name_nothing() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    let lib = temp.path().join("src/lib.rs");
    std::fs::write(
        &lib,
        concat!(
            "use std::collections::HashMap;\n",
            "pub struct Known;\n",
            "pub fn a(_x: NoSuchType) -> Known { Known }\n",
            "pub fn b() -> std::collections::NoSuchMap { todo!() }\n",
            "pub fn c() -> no_such_crate::Thing { todo!() }\n",
            "pub fn d() { let _y: NoSuchLocal = 1; let _z = no_such_fn(); }\n",
            "pub fn e<T: Iterator>(t: T) -> Option<T::Item> {\n",
            "    let m: HashMap<u8, Known> = HashMap::new();\n",
            "    drop(m);\n",
            "    let mut t = t;\n",
            "    t.next()\n",
            "}\n",
            "#[cfg(windows)]\n",
            "pub fn f() -> WindowsOnly { todo!() }\n",
            "#[allow(dead_code)]\n",
            "fn g() -> Vec<Known> { vec![] }\n",
            "pub fn h() -> crate::nowhere::Thing { todo!() }\n",
            "#[test]\n",
            "fn i() {\n",
            "    use std::sync::Mutex;\n",
            "    let m: Mutex<u8> = Mutex::new(0);\n",
            "    drop(m);\n",
            "}\n",
        ),
    )
    .unwrap();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let diagnostics = engine.diagnostics(&lib).unwrap();
    let found: Vec<(u32, &str, &str)> = diagnostics
        .iter()
        .filter(|d| d.code == "unresolved-path")
        .map(|d| {
            let name = d.message.split('`').nth(1).unwrap_or("");
            (d.line, name, d.severity.as_str())
        })
        .collect();
    // Only the paths that name nothing; `T::Item`, the `#[cfg(windows)]` item, the
    // attribute and the call rust-analyzer already reports are not among them. A name
    // missing from another crate is a warning: that crate may hold code the analyzer
    // cannot see.
    assert_eq!(
        found,
        vec![
            (3, "NoSuchType", "error"),
            (4, "NoSuchMap", "warning"),
            (5, "no_such_crate", "error"),
            (6, "NoSuchLocal", "error"),
            (17, "nowhere", "error"),
        ],
        "{diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| d.code == "unresolved-path" || !d.message.contains("NoSuch")),
        "each name is reported once: {diagnostics:?}"
    );
}
