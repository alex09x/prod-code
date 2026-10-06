/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Tests for refactoring operations: rename, safe delete, code assists, and macro restrictions.

use super::create_test_fixture;
use crate::engine::RustEngine;

#[test]
fn test_rename_rewrites_definition_and_uses() {
    let (temp, lib_path) = create_test_fixture();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");
    // `pub struct PathTranslator {` is line 3; the name starts at column 12.
    let outcome = engine
        .rename(&lib_path, 3, 12, "PathMapper")
        .expect("rename query")
        .expect("rename accepted");
    assert_eq!(outcome.files.len(), 1);
    let file = &outcome.files[0];
    assert!(
        file.new_text.contains("pub struct PathMapper {"),
        "{}",
        file.new_text
    );
    assert!(
        file.new_text.contains("impl PathMapper {"),
        "{}",
        file.new_text
    );
    assert!(!file.new_text.contains("PathTranslator"));
    assert_eq!(file.edits, 2);
    assert!(file.old_line_count >= 10);
    assert!(outcome.moves.is_empty());

    // Not a symbol: rust-analyzer refuses instead of the engine erroring.
    let refused = engine.rename(&lib_path, 1, 1, "x").expect("rename query");
    assert!(refused.is_err());
}

#[test]
fn test_safe_delete_refuses_used_and_removes_unused() {
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
        "pub fn used() -> u8 {\n    1\n}\n\npub fn unused() -> u8 {\n    2\n}\n\npub fn caller() -> u8 {\n    used()\n}\n",
    )
    .unwrap();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");

    let refused = engine.safe_delete(&lib, 1, 8).unwrap().unwrap_err();
    assert!(refused.contains("1 usage(s)"), "{refused}");
    assert!(refused.contains("lib.rs:10:"), "{refused}");

    // Inside a body, on no item's name: refused, and nothing of the body goes (#138).
    let nothing = engine.safe_delete(&lib, 6, 5).unwrap().unwrap_err();
    assert!(nothing.contains("no deletable item is named"), "{nothing}");

    let outcome = engine
        .safe_delete(&lib, 5, 8)
        .unwrap()
        .expect("unused item deletes");
    let text = &outcome.files[0].new_text;
    assert!(!text.contains("unused"), "{text}");
    assert!(text.contains("pub fn used()") && text.contains("pub fn caller()"));
    assert!(!text.contains("\n\n\n"), "{text:?}");
}

#[test]
fn test_assists_list_and_apply_inline_variable() {
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
        "pub fn f() -> i32 {\n    let value = 40 + 2;\n    value\n}\n",
    )
    .unwrap();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");

    // Cursor on `value` in `let value = ...` (line 2, col 9).
    let offered = engine.list_assists(&lib, 2, 9, None).unwrap();
    assert!(
        offered.iter().any(|a| a.id == "inline_local_variable"),
        "{offered:?}"
    );

    let outcome = engine
        .apply_assist(&lib, 2, 9, None, "inline_local_variable", None)
        .unwrap()
        .expect("assist applies");
    assert_eq!(outcome.files.len(), 1);
    let text = &outcome.files[0].new_text;
    assert!(!text.contains("let value"), "{text}");
    assert!(text.contains("40 + 2"), "{text}");

    let refused = engine
        .apply_assist(&lib, 2, 9, None, "no_such_assist", None)
        .unwrap();
    let reason = refused.expect_err("refused");
    assert!(reason.contains("not offered here; available"), "{reason}");
    assert!(!reason.contains("macro"), "not inside a macro: {reason}");
}

/// Inside a macro call's input most refactorings are not offered; the refusal says which
/// macro and what to do, rather than only "not offered here" (#99).
#[test]
fn an_assist_refused_inside_a_macro_call_names_the_macro() {
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
        "macro_rules! wrap {\n    ($($t:tt)*) => { $($t)* };\n}\n\npub fn g() -> i32 {\n    wrap! {\n        let y = 1 + 2;\n    }\n    0\n}\n",
    )
    .unwrap();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let refused = engine
        .apply_assist(&lib, 7, 17, Some((7, 22)), "extract_function", None)
        .unwrap();
    let reason = refused.expect_err("nothing is extracted inside a macro call");
    assert!(reason.contains("inside `wrap!`"), "{reason}");
    assert!(
        reason.contains("Move the code out of the macro"),
        "{reason}"
    );
}
