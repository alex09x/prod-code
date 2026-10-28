/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use prod_code_testkit::Workspace;

#[test]
fn go_constructor_insertion_preserves_existing_and_multiline_fields() {
    let single = "Store{}";
    assert_eq!(
        go_constructor_insertion(single, "Store".len(), "Count", "1"),
        Some("Count: 1".to_string())
    );

    let existing = "Store{Count: 2}";
    assert_eq!(
        go_constructor_insertion(existing, "Store".len(), "Count", "1"),
        None
    );

    let multiline = "Store{\n\tName: \"unit\",\n}";
    assert_eq!(
        go_constructor_insertion(multiline, "Store".len(), "Count", "1"),
        Some("\n\tCount: 1,".to_string())
    );
}

#[test]
fn go_constructor_insertion_ignores_incomplete_literals() {
    let text = "Store{";
    assert_eq!(
        go_constructor_insertion(text, "Store".len(), "Count", "0"),
        None
    );
}

#[test]
fn constructor_counts_are_language_and_identifier_specific() {
    for (lang, extension, text, expected) in [
        (Language::TypeScript, "tsx", "new Store(); new Store(1);", 2),
        (Language::JavaScript, "mjs", "new Store();", 1),
        (Language::Python, "py", "Store(); OtherStore(); Store(1)", 2),
        (
            Language::Swift,
            "swift",
            "Store(); OtherStore(); Store(1)",
            2,
        ),
        (
            Language::Cpp,
            "cpp",
            "Store(); Store{}; OtherStore(); OtherStore{}",
            2,
        ),
        (Language::C, "c", "Store(); Store{}; OtherStore();", 2),
        (Language::Rust, "rs", "Store(); Store{}", 0),
        (Language::Java, "java", "new Store();", 0),
    ] {
        let path = format!("source.{extension}");
        let ws = Workspace::new(&[(&path, text), ("ignored.txt", "new Store(); Store{}")]);
        let mut edits = BTreeMap::new();
        let mut texts = BTreeMap::new();
        let mut unmatched = vec![];
        assert_eq!(
            find_constructors(
                &ws.root(),
                &ws.root().join(&path),
                text,
                lang,
                "Store",
                "value",
                "0",
                &mut edits,
                &mut texts,
                &mut unmatched
            ),
            expected,
            "{lang:?}"
        );
        assert_eq!(edits, BTreeMap::new());
        assert_eq!(texts, BTreeMap::new());
        assert_eq!(unmatched, Vec::<String>::new());
    }
}
