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

use crate::engine::RustEngine;
use crate::tests::configure_test_workspace;

const LIB: &str = "pub fn leaf() -> Option<u32> {\n    Some(1)\n}\n\npub fn first() -> u32 {\n    leaf().unwrap()\n}\n\npub fn second() -> u32 {\n    leaf().unwrap() + 1\n}\n\nmod other;\nmod consts;\n";
const OTHER: &str = "pub fn third() -> u32 {\n    crate::leaf().unwrap()\n}\n";
const CONSTS: &str = "pub const ONE: u32 = 1;\npub const TWO: u32 = ONE + 1;\n";

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    engine: RustEngine,
}

fn fixture(lib: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    configure_test_workspace(&root);
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), lib).unwrap();
    std::fs::write(root.join("src/other.rs"), OTHER).unwrap();
    std::fs::write(root.join("src/consts.rs"), CONSTS).unwrap();
    let engine = RustEngine::load(&root).expect("fixture loads");
    Fixture {
        _dir: dir,
        root,
        engine,
    }
}

fn replaced(
    f: &Fixture,
    rule: &str,
    ctx: &Path,
    pos: (u32, u32),
    scope: Option<&Path>,
) -> Result<crate::types::RefactorOutcome, String> {
    f.engine
        .structural_replace(rule, ctx, pos.0, pos.1, scope)
        .expect("position and files are valid")
}

#[test]
fn rewrites_every_match_in_the_workspace_when_unscoped() {
    let f = fixture(LIB);
    let lib = f.root.join("src/lib.rs");
    let out = replaced(
        &f,
        "$e.unwrap() ==>> $e.expect(\"present\")",
        &lib,
        (1, 1),
        None,
    )
    .unwrap();
    let mut files: Vec<_> = out
        .files
        .iter()
        .map(|r| (r.path.clone(), r.edits))
        .collect();
    files.sort();
    assert_eq!(
        files,
        vec![
            (f.root.join("src/lib.rs"), 2),
            (f.root.join("src/other.rs"), 1)
        ]
    );
    assert_eq!(out.total_edits(), 3);
    let lib_new = &out.files.iter().find(|r| r.path == lib).unwrap().new_text;
    assert_eq!(
        lib_new,
        &LIB.replace("leaf().unwrap()", "leaf().expect(\"present\")")
    );
    assert!(out.created.is_empty() && out.moves.is_empty());
}

#[test]
fn scope_limits_the_rewrite_to_one_file() {
    let f = fixture(LIB);
    let lib = f.root.join("src/lib.rs");
    let other = f.root.join("src/other.rs");
    let out = replaced(
        &f,
        "$e.unwrap() ==>> $e.expect(\"present\")",
        &lib,
        (6, 5),
        Some(&other),
    )
    .unwrap();
    assert_eq!(out.files.len(), 1);
    assert_eq!(out.files[0].path, other);
    assert_eq!(out.files[0].edits, 1);
    assert_eq!(
        out.files[0].new_text,
        "pub fn third() -> u32 {\n    crate::leaf().expect(\"present\")\n}\n"
    );
}

#[test]
fn a_rule_without_matches_changes_nothing() {
    let f = fixture(LIB);
    let lib = f.root.join("src/lib.rs");
    let out = replaced(&f, "$e.unwrap_or_default() ==>> $e", &lib, (1, 1), None).unwrap();
    assert!(out.files.is_empty());
    assert_eq!(out.total_edits(), 0);
}

#[test]
fn a_malformed_rule_is_refused_with_the_analyzer_message() {
    let f = fixture(LIB);
    let lib = f.root.join("src/lib.rs");
    let err = replaced(&f, "not a rule", &lib, (1, 1), None).unwrap_err();
    assert!(err.contains("==>>"), "{err}");
}

#[test]
fn bad_context_scope_and_position_are_errors() {
    let f = fixture(LIB);
    let lib = f.root.join("src/lib.rs");
    let rule = "$e.unwrap() ==>> $e";
    let missing = f.root.join("src/missing.rs");
    assert!(
        f.engine
            .structural_replace(rule, &missing, 1, 1, None)
            .is_err()
    );
    assert!(
        f.engine
            .structural_replace(rule, &lib, 1, 1, Some(&missing))
            .is_err()
    );
    let err = f
        .engine
        .structural_replace(rule, &lib, 999, 1, None)
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("Invalid position 999:1"),
        "{err:#}"
    );
}

#[test]
fn a_file_without_functions_still_resolves_its_own_paths() {
    let f = fixture(LIB);
    let consts = f.root.join("src/consts.rs");
    let out = replaced(&f, "ONE ==>> 5", &consts, (1, 1), Some(&consts)).unwrap();
    assert_eq!(out.files.len(), 1);
    assert_eq!(
        out.files[0].new_text,
        "pub const ONE: u32 = 1;\npub const TWO: u32 = 5 + 1;\n"
    );
}

#[test]
fn coverage_w3_first_body_offset_selects_scoped_tokens() {
    for (source, selected) in [
        (
            "trait T { fn required(); }",
            Some("trait T { fn required()"),
        ),
        (
            "pub static VALUE: u32 = 7;",
            Some("pub static VALUE: u32 = 7"),
        ),
        ("pub struct Unit;", Some("pub struct Unit")),
        (
            "mod nested { use core::mem; }",
            Some("mod nested { use core::mem; }"),
        ),
        ("// no scoped items\n", None),
        (
            "trait T { fn required(); }\nfn actual() {}",
            Some("trait T { fn required(); }\nfn actual() {}"),
        ),
    ] {
        let f = fixture(source);
        let snapshot = f.engine.snapshot();
        let file = snapshot
            .require_file_id(&f.root.join("src/lib.rs"))
            .unwrap();
        assert_eq!(
            snapshot.first_body_offset(file, source).map(u32::from),
            selected.map(|text| text.len() as u32),
            "{source}"
        );
    }
}
