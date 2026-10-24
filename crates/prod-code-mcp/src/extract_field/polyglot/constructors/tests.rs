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
fn go_literals_receive_initializers_only_in_the_selected_package() {
    let owner = "package sample\ntype Store struct {}\n";
    let literals = "package sample\nvar a = Store{}\nvar b = Store{Size: 2}\nvar c = Store{\n\tSize: 3,\n}\nvar d = Store{\n}\nvar e = Store{Count: 4}\n// Store{}\nvar s = `Store{}`\nvar other = OtherStore{}\nvar qualified = foreign.Store{}\nvar broken = Store{\n";
    let ws = Workspace::new(&[
        ("owner.go", owner),
        ("uses.go", literals),
        ("other.go", "package alternate\nvar a = Store{}\n"),
        ("nested/use.go", "package sample\nvar a = Store{}\n"),
        ("ignored.txt", "Store{}"),
    ]);
    let root = ws.root();
    let file = root.join("owner.go");
    let mut edits = BTreeMap::new();
    let mut texts = BTreeMap::new();
    let mut unmatched = vec![];
    let count = find_constructors(
        &root,
        &file,
        owner,
        Language::Go,
        "Store",
        "Count",
        "1",
        &mut edits,
        &mut texts,
        &mut unmatched,
    );
    assert_eq!(count, 4);
    let uses = root.join("uses.go");
    let expected: Vec<_> = [
        ("Store{}", "Count: 1"),
        ("Store{Size: 2}", "Count: 1, "),
        ("Store{\n\tSize: 3,\n}", "\n\tCount: 1,"),
        ("Store{\n}", "\n        Count: 1,"),
    ]
    .into_iter()
    .map(|(needle, insert)| {
        (
            literals.find(needle).unwrap() + "Store{".len(),
            0,
            insert.to_string(),
        )
    })
    .collect();
    assert_eq!(edits, BTreeMap::from([(uses.clone(), expected)]));
    assert_eq!(texts, BTreeMap::from([(uses, literals.to_string())]));
    unmatched.sort();
    assert_eq!(
        unmatched,
        [
            "nested/use.go:2: unqualified Go literal type `Store` is in another package and cannot be resolved safely",
            "other.go:2: unqualified Go literal type `Store` is in another package and cannot be resolved safely",
            "uses.go:13: Go literal type `Store` is package-qualified and cannot be matched to the selected declaration",
        ]
    );
}

#[test]
fn missing_go_package_refuses_unqualified_constructors() {
    let text = "var a = Store{}\n";
    let ws = Workspace::new(&[("owner.go", text)]);
    let mut edits = BTreeMap::new();
    let mut texts = BTreeMap::new();
    let mut unmatched = vec![];
    assert_eq!(
        find_constructors(
            &ws.root(),
            &ws.root().join("owner.go"),
            text,
            Language::Go,
            "Store",
            "Count",
            "0",
            &mut edits,
            &mut texts,
            &mut unmatched
        ),
        0
    );
    assert_eq!(
        unmatched,
        [
            "owner.go:1: unqualified Go literal type `Store` is in another package and cannot be resolved safely"
        ]
    );
    assert_eq!(edits, BTreeMap::new());
    assert_eq!(texts, BTreeMap::new());
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
