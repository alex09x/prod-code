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
#[test]
fn nearest_manifest_is_fallback_but_git_root_wins() {
    for manifest in [
        "Cargo.toml",
        "go.mod",
        "package.json",
        "pyproject.toml",
        "Package.swift",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join(manifest), "").unwrap();
        let file = nested.join("source.txt");
        std::fs::write(&file, "text").unwrap();
        assert_eq!(find_workspace_root(&file), Some(nested.clone()));
        std::fs::write(root.join(".git"), "gitdir: placeholder").unwrap();
        assert_eq!(find_workspace_root(&file), Some(root));
    }
}
#[test]
fn first_declaration_table() {
    for (source, expected) in [
        (
            "// comment\r\n\r\npub const VALUE: u32 = 1;\r\n",
            Some((2, 12)),
        ),
        (
            "# comment\n/* comment */\npub struct Value;\n",
            Some((2, 13)),
        ),
        ("\npub fn value() {}\n", Some((1, 9))),
        ("func Value() {}\n", Some((0, 6))),
        ("let value = 1;\n", None),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.rs");
        std::fs::write(&path, source).unwrap();
        assert_eq!(
            find_first_code_file(dir.path()),
            expected.map(|(line, col)| (path, line, col))
        );
    }
}
