/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{APPLE, SINGLE, assert_routes, fixture, write};

#[tokio::test]
async fn issue_1330_pruning_modern_manifest_graph() {
    // Modern roots contribute immediate requirements; an unimported, non-root leaf's
    // manifest must not upgrade the Apple dependency. Legacy graphs retain the closure.
    for (main_go, bridge_go, root_leaf, expected) in [
        ("1.24", "1.24", false, Some("apple/apple.go")),
        ("1.17", "1.17", false, Some("apple/apple.go")),
        ("1.24", "1.16", false, None),
        ("1.16", "1.24", false, None),
        ("1.24", "1.24", true, None),
    ] {
        let leaf = if root_leaf {
            "require example.org/leaf v1.0.0\n"
        } else {
            ""
        };
        let manifest = format!(
            "module example.org/root\ngo {main_go}\nrequire (\n example.org/apple v1.0.0\n example.org/bridge v1.0.0\n)\n{leaf}replace (\n example.org/apple v1.0.0 => ./apple\n example.org/apple v1.1.0 => ./portable\n example.org/bridge => ./bridge\n example.org/leaf => ./leaf\n)\n"
        );
        let temp = fixture(&manifest, "package main\nimport _ \"example.org/apple\"\n");
        let root = temp.path();
        write(root, "apple/go.mod", "module example.org/apple\ngo 1.24\n");
        write(
            root,
            "portable/go.mod",
            "module example.org/apple\ngo 1.24\n",
        );
        write(root, "portable/apple.go", "package apple\n");
        write(
            root,
            "bridge/go.mod",
            &format!(
                "module example.org/bridge\ngo {bridge_go}\nrequire example.org/leaf v1.0.0\n"
            ),
        );
        write(
            root,
            "leaf/go.mod",
            "module example.org/leaf\ngo 1.24\nrequire example.org/apple v1.1.0\n",
        );
        write(root, "bridge/bridge.go", APPLE);
        write(root, "leaf/leaf.go", APPLE);
        assert_routes(root, expected).await;
    }
}

#[tokio::test]
async fn issue_1330_pruning_ignored_filename_imports() {
    for name in [
        "_scratch.go",
        ".scratch.go",
        "main_windows.go",
        "main_windows_amd64.go",
        "main_freebsd.go",
        "main_android.go",
        "main_plan9_test.go",
        "main_386.go",
        "main_linux_riscv64.go",
        "main_wasm.go",
    ] {
        let temp = fixture(SINGLE, "package main\n");
        write(
            temp.path(),
            name,
            "package main\nimport _ \"example.org/apple\"\n",
        );
        assert_routes(temp.path(), None).await;
    }
    // Both supported Linux architectures and non-special suffixes still follow imports.
    for name in [
        "main_linux.go",
        "main_amd64.go",
        "main_linux_arm64_test.go",
        "main_custom.go",
    ] {
        let temp = fixture(SINGLE, "package main\n");
        write(
            temp.path(),
            name,
            "package main\nimport _ \"example.org/apple\"\n",
        );
        assert_routes(temp.path(), Some("apple/apple.go")).await;
    }
}

#[tokio::test]
async fn issue_1330_pruning_selected_module_import_owner() {
    for import in ["example.org/tree/app", "example.org/tree/app/sub"] {
        let manifest = "module example.org/tree/app\ngo 1.24\nrequire example.org/tree v1.0.0\nreplace example.org/tree => ./dep\n";
        let temp = fixture(manifest, "package app\n");
        write(
            temp.path(),
            "cmd/tool/main.go",
            &format!("package main\nimport _ \"{import}\"\n"),
        );
        write(temp.path(), "sub/sub.go", "package sub\n");
        write(
            temp.path(),
            "dep/go.mod",
            "module example.org/tree\ngo 1.24\n",
        );
        write(temp.path(), "dep/apple.go", APPLE);
        assert_routes(temp.path(), None).await;
    }
    // A genuinely more specific required module still owns its imported package.
    let manifest = "module example.org/tree\ngo 1.24\nrequire example.org/tree/app v1.0.0\nreplace example.org/tree/app => ./apple\n";
    let temp = fixture(
        manifest,
        "package main\nimport _ \"example.org/tree/app/sub\"\n",
    );
    write(
        temp.path(),
        "apple/go.mod",
        "module example.org/tree/app\ngo 1.24\n",
    );
    write(temp.path(), "apple/sub/sub.go", "package sub\n");
    assert_routes(temp.path(), Some("apple/apple.go")).await;
}
