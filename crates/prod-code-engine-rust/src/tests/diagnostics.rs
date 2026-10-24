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
            "use root_pkg::TopItem;\n",
            "fn t() {\n",
            "    use std::sync::Mutex;\n",
            "    { let m: Mutex<u8> = todo!(); let h: HashMap<u8, u8> = todo!(); }\n",
            "    mod block_child { fn bc(_: Mutex<u8>) {} }\n",
            "}\n",
            "fn u() { let m: Mutex<u8> = todo!(); let top: TopItem = todo!(); }\n",
            "mod m {\n",
            "    use opaque::Driver;\n",
            "    fn v() { let d: Driver = todo!(); let t: TopItem = todo!(); }\n",
            "    mod nested { fn nv(_: Driver) {} }\n",
            "}\n",
            "fn w() { let d: Driver = todo!(); }\n",
            "mod aliases {\n",
            "    use alias_mod::Orig as BoundAlias;\n",
            "    use anon_mod::AnonTrait as _;\n",
            "    use pkg::grouped::{self, GroupOrig as GroupAlias, Plain};\n",
            "    use pkg::self_renamed::{self as SelfAlias};\n",
            "    use pkg::self_path::self;\n",
            "    fn check(_: BoundAlias) {}\n",
            "}\n",
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
    // An enclosing block's `use` does not cross into an inline module inside that block.
    assert!(!imported_in_scope(segment(2, "Mutex").syntax(), "Mutex"));
    // Another function's `use` does not reach here, while the file-level `use` does.
    assert!(!imported_in_scope(segment(3, "Mutex").syntax(), "Mutex"));
    assert!(imported_in_scope(segment(1, "TopItem").syntax(), "TopItem"));
    // A module's `use` reaches its functions, not nested modules or functions outside it,
    // and a file-level `use` does not leak into `mod m`.
    assert!(imported_in_scope(segment(1, "Driver").syntax(), "Driver"));
    assert!(!imported_in_scope(segment(1, "Driver").syntax(), "opaque"));
    assert!(!imported_in_scope(
        segment(2, "TopItem").syntax(),
        "TopItem"
    ));
    assert!(!imported_in_scope(segment(2, "Driver").syntax(), "Driver"));
    assert!(!imported_in_scope(segment(3, "Driver").syntax(), "Driver"));
    // Renamed, underscore, and grouped `self` imports bind only their introduced names.
    let in_aliases_seg = segment(0, "BoundAlias");
    let in_aliases = in_aliases_seg.syntax();
    assert!(imported_in_scope(in_aliases, "BoundAlias"));
    assert!(!imported_in_scope(in_aliases, "Orig"));
    assert!(!imported_in_scope(in_aliases, "alias_mod"));
    assert!(!imported_in_scope(in_aliases, "AnonTrait"));
    assert!(!imported_in_scope(in_aliases, "anon_mod"));
    assert!(imported_in_scope(in_aliases, "grouped"));
    assert!(imported_in_scope(in_aliases, "GroupAlias"));
    assert!(imported_in_scope(in_aliases, "Plain"));
    assert!(!imported_in_scope(in_aliases, "GroupOrig"));
    assert!(!imported_in_scope(in_aliases, "pkg"));
    assert!(imported_in_scope(in_aliases, "SelfAlias"));
    assert!(!imported_in_scope(in_aliases, "self_renamed"));
    assert!(imported_in_scope(in_aliases, "self_path"));
}

#[test]
fn test_diagnostics_report_paths_that_name_nothing() {
    let temp = tempfile::tempdir().unwrap();
    super::configure_test_workspace(temp.path());
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

#[test]
fn test_diagnostics_respect_module_boundaries_import_bindings_and_inline_uses() {
    let temp = tempfile::tempdir().unwrap();
    super::configure_test_workspace(temp.path());
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
            "mod m { pub struct X; pub struct Z; }\n",
            "mod a { pub mod sub { pub struct Q; } }\n",
            "use m::Z;\n",
            "use m::X as Y;\n",
            "use a::sub::Q;\n",
            "use crate::missing::Orig as Alias;\n",
            "use crate::missing_group::self as GroupAlias;\n",
            "pub fn root_z(_: Z) {}\n",
            "pub mod child {\n",
            "    pub fn f(_: Z) {}\n",
            "}\n",
            "pub mod parent_import {\n",
            "    use super::m::Z;\n",
            "    pub fn ok(_: Z) {}\n",
            "    pub mod nested {\n",
            "        pub fn g(_: Z) {}\n",
            "    }\n",
            "}\n",
            "pub fn wrong_orig(_: X) -> Y { Y }\n",
            "pub fn wrong_mod(_: sub::Q) -> Q { Q }\n",
            "pub fn uses_unresolved_import(_: Alias, _: GroupAlias, _: Orig, _: missing_group) {}\n",
            "#[test]\n",
            "fn macro_expanded_alias_and_group() {\n",
            "    use std::sync::{self, Mutex as Lock};\n",
            "    let m: Lock<u8> = sync::Mutex::new(0);\n",
            "    drop(m);\n",
            "}\n",
            "use std::fmt; use std::collections::HashMap;\n",
            "#[allow(dead_code)] use std::collections::BTreeMap;\n",
            "pub fn fmt_user(_: &dyn fmt::Debug) {}\n",
            "use missing_grouped::{UnusedA, UnusedB};\n",
            "use crate::missing_grouped_local::{UnusedC, UnusedD};\n",
            "pub fn uses_grouped(_: UnusedA, _: UnusedB, _: UnusedC, _: UnusedD) {}\n",
        ),
    )
    .unwrap();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let diagnostics = engine.diagnostics(&lib).unwrap();
    let unresolved: Vec<(u32, u32, u32, u32, &str)> = diagnostics
        .iter()
        .filter(|d| d.code == "unresolved-path")
        .map(|d| {
            let name = d.message.split('`').nth(1).unwrap_or("");
            (d.line, d.col, d.end_line, d.end_col, name)
        })
        .collect();
    let unused: Vec<(u32, u32, u32, u32)> = diagnostics
        .iter()
        .filter(|d| d.code == "unused_imports")
        .map(|d| (d.line, d.col, d.end_line, d.end_col))
        .collect();
    assert_eq!(
        (unresolved, unused),
        (
            vec![
                (10, 17, 10, 18, "Z"),
                (16, 21, 16, 22, "Z"),
                (19, 22, 19, 23, "X"),
                (20, 21, 20, 24, "sub"),
                (21, 59, 21, 63, "Orig"),
                (21, 68, 21, 81, "missing_group"),
            ],
            vec![(28, 15, 28, 45), (29, 21, 29, 52)],
        ),
        "{diagnostics:?}"
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| d.code == "E0432" && matches!(d.line, 31 | 32))
            .map(|d| (d.line, d.col, d.end_line, d.end_col))
            .collect::<Vec<_>>(),
        vec![
            (31, 23, 31, 30),
            (31, 32, 31, 39),
            (32, 36, 32, 43),
            (32, 45, 32, 52)
        ],
        "native unresolved-import errors must remain visible"
    );
}
