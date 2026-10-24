/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use std::path::Path;

use super::super::packages::{go_package_name, package_sources, type_declarations};

fn err<T>(r: anyhow::Result<T>) -> String {
    r.err().expect("an error").to_string()
}

#[test]
fn the_package_clause_is_read_past_comments() {
    let cases = [
        ("package p\n", Some("p")),
        ("// c\n/* x */ package  main\n", Some("main")),
        ("package /* c */ q\n", Some("q")),
        ("packages p\n", None),
        ("", None),
        ("package", None),
        ("package 1x\n", None),
        ("var package\n", None),
        ("/* open\npackage p\n", None),
    ];
    for (text, want) in cases {
        assert_eq!(go_package_name(text), want, "{text:?}");
    }
}

#[test]
fn a_package_is_the_go_files_of_its_directory_with_the_same_package_clause() {
    let dir = tempfile::Builder::new().prefix("gp").tempdir().unwrap();
    let root = dir.path();
    for (name, text) in [
        ("a.go", "package p\n// disk\n"),
        ("b.go", "package p\n"),
        ("c.go", "package q\n"),
        ("d.txt", "package p\n"),
        ("e.go", "// no clause\n"),
    ] {
        std::fs::write(root.join(name), text).unwrap();
    }
    std::fs::create_dir(root.join("dir.go")).unwrap();
    let sources = package_sources(&root.join("a.go"), "package p\n// edited\n").unwrap();
    let got: Vec<(String, &str)> = sources
        .iter()
        .map(|(path, text)| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                text.as_str(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("a.go".to_string(), "package p\n// edited\n"),
            ("b.go".to_string(), "package p\n")
        ]
    );
}

#[test]
fn a_package_that_cannot_be_inspected_is_refused() {
    let dir = tempfile::Builder::new().prefix("gp").tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.go"), "package p\n").unwrap();
    assert_eq!(
        err(package_sources(Path::new("/"), "package p\n")),
        "/ has no containing package directory"
    );
    assert_eq!(
        err(package_sources(&root.join("a.go"), "func f() {}\n")),
        format!(
            "cannot identify the Go package in {}",
            root.join("a.go").display()
        )
    );
    let gone = root.join("nodir/a.go");
    assert_eq!(
        err(package_sources(&gone, "package p\n")),
        format!(
            "cannot inspect Go package directory {}",
            root.join("nodir").display()
        )
    );
    assert_eq!(
        err(package_sources(&root.join("other.go"), "package p\n")),
        "the declaring Go source disappeared from its package"
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("a.go"), root.join("link.go")).unwrap();
        assert_eq!(
            err(package_sources(&root.join("a.go"), "package p\n")),
            format!(
                "linked Go source {} cannot be inspected",
                root.join("link.go").display()
            )
        );
    }
}

#[test]
fn package_level_type_declarations_are_found_with_their_alias_and_generic_shape() {
    const PLAIN: (bool, bool) = (false, false);
    const ALIAS: (bool, bool) = (true, false);
    const GENERIC: (bool, bool) = (false, true);
    let cases: &[(&str, &str, Vec<(bool, bool)>)] = &[
        ("type T struct{}\n", "T", vec![PLAIN]),
        ("type T = int\n", "T", vec![ALIAS]),
        ("type T[K comparable, V any] map[K]V\n", "T", vec![GENERIC]),
        ("type T[P any] = []P\n", "T", vec![(true, true)]),
        ("type T [N]int\n", "T", vec![PLAIN]),
        ("type T []int\n", "T", vec![PLAIN]),
        ("type T [...]int\n", "T", vec![PLAIN]),
        ("type T [N*2]int\n", "T", vec![PLAIN]),
        ("type T [N+1]int\n", "T", vec![PLAIN]),
        ("type T [pkg.N]int\n", "T", vec![PLAIN]),
        ("type T [f(a, b)]int\n", "T", vec![PLAIN]),
        ("type T [(1)]int\n", "T", vec![PLAIN]),
        ("type T [P ~int]P\n", "T", vec![GENERIC]),
        ("type T [P []int]P\n", "T", vec![GENERIC]),
        ("type T [P interface{}]P\n", "T", vec![GENERIC]),
        ("type T [P é]P\n", "T", vec![GENERIC]),
        ("type T /* c */ int\n", "T", vec![PLAIN]),
        ("type T int\ntype T string\n", "T", vec![PLAIN, PLAIN]),
        ("type T int\ntype U int\n", "U", vec![PLAIN]),
        (
            "type (\n\tA int\n\tT struct {\n\t\tT int\n\t}\n\tU = V\n)\n",
            "T",
            vec![PLAIN],
        ),
        ("type (\n\tA int\n\tU = V\n)\n", "U", vec![ALIAS]),
        ("type ( A int; T = X )\n", "T", vec![ALIAS]),
        ("type (\n\t// T int\n\tT int\n)\n", "T", vec![PLAIN]),
        ("type (\n\tA [2]int\n\tT int\n)\n", "T", vec![PLAIN]),
        (
            "type (\n\tA func(\n\t\tT int,\n\t)\n\tB int\n)\n",
            "T",
            vec![],
        ),
        ("func f() {\n\ttype T int\n}\n", "T", vec![]),
        ("// type T int\nconst s = \"type T int\"\n", "T", vec![]),
        ("typeof := 1\n", "T", vec![]),
        ("type", "T", vec![]),
        ("type (\n\tT int\n", "T", vec![]),
        ("type T /* open", "T", vec![GENERIC]),
        ("type T[", "T", vec![GENERIC]),
        ("type T[P any /* open", "T", vec![GENERIC]),
        ("type T[P any] /* open", "T", vec![GENERIC]),
    ];
    for (text, wanted, want) in cases {
        assert_eq!(type_declarations(text, wanted), *want, "{text:?}");
    }
}
