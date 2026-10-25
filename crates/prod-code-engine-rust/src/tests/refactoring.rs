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

#[test]
fn ssr_implicit_context_resolves_receiver_method_and_item_scopes() {
    let temp = tempfile::tempdir().unwrap();
    super::configure_test_workspace(temp.path());
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let src_dir = temp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let lib = src_dir.join("lib.rs");
    let other = src_dir.join("other.rs");
    std::fs::write(
        &lib,
        "mod other;\nmod defs {\n    pub const FALLBACK: u32 = 1;\n}\npub mod inner {\n    pub struct S(pub u32);\n    impl S {\n        pub const VALUE: u32 = 1;\n        pub fn value(&self) -> u32 {\n            use crate::defs::FALLBACK;\n            Self::VALUE + S::VALUE + self.0 + FALLBACK\n        }\n    }\n}\n",
    )
    .unwrap();
    std::fs::write(
        &other,
        "mod inner {\n    pub struct Item;\n    impl Item {\n        pub const VALUE: u32 = 1;\n    }\n    pub const CURRENT: u32 = Item::VALUE;\n}\n",
    )
    .unwrap();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");

    // Explicit method position (line 11, col 13 inside `fn value(&self)`) resolves `S::VALUE`,
    // method-body import `FALLBACK`, and receiver `self.0`, while respecting `scope: Some(&lib)`.
    let explicit_assoc = engine
        .structural_replace("S::VALUE ==>> 2", &lib, 11, 13, Some(&lib))
        .unwrap()
        .expect("explicit method position resolves S::VALUE");
    assert_eq!(explicit_assoc.files.len(), 1);
    assert_eq!(explicit_assoc.files[0].path, lib);
    assert!(
        explicit_assoc.files[0]
            .new_text
            .contains("Self::VALUE + 2 + self.0 + FALLBACK"),
        "{}",
        explicit_assoc.files[0].new_text
    );

    let explicit_body_import = engine
        .structural_replace("FALLBACK ==>> 2", &lib, 11, 13, Some(&lib))
        .unwrap()
        .expect("explicit method position resolves method-body import");
    assert_eq!(explicit_body_import.files.len(), 1);
    assert_eq!(explicit_body_import.files[0].path, lib);
    assert!(
        explicit_body_import.files[0]
            .new_text
            .contains("Self::VALUE + S::VALUE + self.0 + 2"),
        "{}",
        explicit_body_import.files[0].new_text
    );

    let explicit_self = engine
        .structural_replace("self.0 ==>> 2", &lib, 11, 13, Some(&lib))
        .unwrap()
        .expect("explicit method position resolves receiver self");
    assert_eq!(explicit_self.files.len(), 1);
    assert_eq!(explicit_self.files[0].path, lib);
    assert!(
        explicit_self.files[0]
            .new_text
            .contains("Self::VALUE + S::VALUE + 2 + FALLBACK"),
        "{}",
        explicit_self.files[0].new_text
    );

    // Default (1, 1) context on a file whose only function is a receiver method (`&self`) must
    // select that method's body scope and produce the exact same file-scoped rewrites.
    let implicit_assoc = engine
        .structural_replace("S::VALUE ==>> 2", &lib, 1, 1, Some(&lib))
        .unwrap()
        .expect("implicit (1, 1) context resolves S::VALUE in receiver-method file");
    assert_eq!(implicit_assoc.files, explicit_assoc.files);

    let implicit_body_import = engine
        .structural_replace("FALLBACK ==>> 2", &lib, 1, 1, Some(&lib))
        .unwrap()
        .expect("implicit (1, 1) context resolves method-body import in receiver-method file");
    assert_eq!(implicit_body_import.files, explicit_body_import.files);

    let implicit_self = engine
        .structural_replace("self.0 ==>> 2", &lib, 1, 1, Some(&lib))
        .unwrap()
        .expect("implicit (1, 1) context sets current_function for receiver method");
    assert_eq!(implicit_self.files, explicit_self.files);

    // Default (1, 1) context on an item-only file (no functions or methods) selects a resolvable
    // item scope (`CURRENT` inside `mod inner`) matching an explicit item position.
    let explicit_item_only = engine
        .structural_replace("Item::VALUE ==>> 2", &other, 6, 30, Some(&other))
        .unwrap()
        .expect("explicit item position resolves item-only file scope");
    let implicit_item_only = engine
        .structural_replace("Item::VALUE ==>> 2", &other, 1, 1, Some(&other))
        .unwrap()
        .expect("implicit (1, 1) context resolves item-only file scope");
    assert_eq!(implicit_item_only.files, explicit_item_only.files);
    assert_eq!(implicit_item_only.files.len(), 1);
    assert_eq!(implicit_item_only.files[0].path, other);
    assert!(
        implicit_item_only.files[0]
            .new_text
            .contains("pub const CURRENT: u32 = 2;"),
        "{}",
        implicit_item_only.files[0].new_text
    );
}

#[test]
fn ssr_implicit_context_block_initializers_ignore_trailing_trivia() {
    let temp = tempfile::tempdir().unwrap();
    super::configure_test_workspace(temp.path());
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let src = temp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("lib.rs"),
        "mod defs { pub const FALLBACK: u32 = 1; }\nmod spaced;\nmod commented;\nmod static_spaced;\nmod static_commented;\n",
    )
    .unwrap();
    let cases = [
        ("spaced", "const", "} ;\n"),
        ("commented", "const", "} /* keep */;\n"),
        ("static_spaced", "static", "} ;\n"),
        ("static_commented", "static", "} /* keep */;\n"),
    ];
    for (name, kind, ending) in cases {
        std::fs::write(
            src.join(format!("{name}.rs")),
            format!(
                "pub {kind} CURRENT: u32 = {{\n    use crate::defs::FALLBACK;\n    FALLBACK\n{ending}"
            ),
        )
        .unwrap();
    }
    let engine = RustEngine::load(temp.path()).expect("load block initializer fixture");
    let mut failures = Vec::new();
    for (name, _, _) in cases {
        let path = src.join(format!("{name}.rs"));
        let explicit = engine
            .structural_replace("FALLBACK ==>> 2", &path, 3, 5, Some(&path))
            .unwrap()
            .expect("explicit initializer position resolves its local import");
        assert_eq!(explicit.files.len(), 1);
        assert!(explicit.files[0].new_text.contains("    2\n"));
        match engine
            .structural_replace("FALLBACK ==>> 2", &path, 1, 1, Some(&path))
            .unwrap()
        {
            Ok(implicit) => assert_eq!(implicit.files, explicit.files),
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
