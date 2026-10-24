/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::Language::{self, *};
use super::{collect_workspace_sources, display, is_candidate_source_file, is_ident, mentions};
use std::path::Path;

#[test]
fn language_from_extension() {
    let rows: &[(&str, Option<Language>)] = &[
        ("a.rs", Some(Rust)),
        ("a.py", Some(Python)),
        ("a.ts", Some(TypeScript)),
        ("a.tsx", Some(TypeScript)),
        ("a.js", Some(JavaScript)),
        ("a.jsx", Some(JavaScript)),
        ("a.mjs", Some(JavaScript)),
        ("a.cjs", Some(JavaScript)),
        ("a.go", Some(Go)),
        ("a.c", Some(C)),
        ("a.h", Some(C)),
        ("a.cpp", Some(Cpp)),
        ("a.cc", Some(Cpp)),
        ("a.cxx", Some(Cpp)),
        ("a.hpp", Some(Cpp)),
        ("a.swift", Some(Swift)),
        ("a.java", Some(Java)),
        ("a.kt", Some(Kotlin)),
        ("a.kts", Some(Kotlin)),
        ("a.cs", Some(Csharp)),
        ("a.zig", Some(Zig)),
        ("a.zon", Some(Zig)),
        ("a.RS", None),
        ("a.txt", None),
        ("Makefile", None),
        ("dir/.rs", None),
    ];
    for (file, expected) in rows {
        assert_eq!(Language::of(Path::new(file)), *expected, "{file}");
    }
}

#[test]
fn candidate_files_per_language() {
    let rows: &[(Language, &[&str], &[&str])] = &[
        (TypeScript, &["ts", "tsx", "js", "jsx"], &["py", "mjs"]),
        (JavaScript, &["js", "jsx", "ts", "tsx"], &["go", "cjs"]),
        (Python, &["py"], &["pyi", "rs"]),
        (
            Cpp,
            &["cpp", "cc", "cxx", "c", "h", "hpp", "hxx"],
            &["swift"],
        ),
        (C, &["cpp", "cc", "cxx", "c", "h", "hpp", "hxx"], &["java"]),
        (Swift, &["swift"], &["c"]),
        (Go, &["go"], &["mod"]),
        (Rust, &["rs"], &["toml"]),
        (Java, &["java"], &["kt"]),
        (Kotlin, &["kt", "kts"], &["java"]),
        (Csharp, &["cs"], &["csx"]),
        (Zig, &["zig", "zon"], &["rs"]),
    ];
    for (lang, yes, no) in rows {
        for ext in *yes {
            assert!(
                is_candidate_source_file(Path::new(&format!("x.{ext}")), *lang),
                "{lang:?} {ext}"
            );
        }
        for ext in *no {
            assert!(
                !is_candidate_source_file(Path::new(&format!("x.{ext}")), *lang),
                "{lang:?} {ext}"
            );
        }
        assert!(!is_candidate_source_file(Path::new("noext"), *lang));
    }
}

#[test]
fn workspace_walk_skips_hidden_and_build_directories_and_sorts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for rel in [
        "a.rs",
        "src/b.rs",
        "src/c.py",
        "sub/deep/i.rs",
        "sub/deep/notes.txt",
        ".hidden/d.rs",
        ".dotfile.rs",
        "target/e.rs",
        "node_modules/f.rs",
        "build/g.rs",
        "dist/h.rs",
        "dirlike.rs/inner.rs",
    ] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "fn x() {}\n").unwrap();
    }
    let shown = |lang| -> Vec<String> {
        collect_workspace_sources(root, lang)
            .iter()
            .map(|p| display(root, p))
            .collect()
    };
    assert_eq!(
        shown(Rust),
        ["a.rs", "dirlike.rs/inner.rs", "src/b.rs", "sub/deep/i.rs"]
    );
    assert_eq!(shown(Python), ["src/c.py"]);
    assert!(shown(Go).is_empty());
    assert!(collect_workspace_sources(&root.join("missing"), Rust).is_empty());
}

#[test]
fn identifier_characters_and_display_paths() {
    for c in ['a', 'Z', '0', '_', 'é', 'ж', '日'] {
        assert!(is_ident(c), "{c}");
    }
    for c in [' ', '-', '.', '€', '$', '\n'] {
        assert!(!is_ident(c), "{c:?}");
    }
    let root = Path::new("/ws/repo");
    assert_eq!(display(root, Path::new("/ws/repo/src/a.rs")), "src/a.rs");
    assert_eq!(
        display(root, Path::new("/elsewhere/a.rs")),
        "/elsewhere/a.rs"
    );
    assert_eq!(display(root, root), "");
}

#[test]
fn mentions_requires_an_isolated_identifier() {
    let rows: &[(&str, &str, bool)] = &[
        ("let total = 1;", "total", true),
        ("total", "total", true),
        ("(total)", "total", true),
        ("subtotal + 1", "total", false),
        ("total2 + 1", "total", false),
        ("total_x", "total", false),
        ("_total", "total", false),
        ("subtotal + total", "total", true),
        ("totalé", "total", false),
        ("étotal", "total", false),
        ("日 total 日", "total", true),
        ("日total", "total", false),
        ("", "total", false),
    ];
    for (text, word, expected) in rows {
        assert_eq!(mentions(text, word), *expected, "`{word}` in `{text}`");
    }
}
