/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{APPLE, Node, SINGLE, fixture};
use crate::cluster::{checkout_node_in, route_in};

#[path = "route_local_deps_pruning.rs"]
mod pruning;

#[path = "route_local_deps_literals.rs"]
mod literals;

#[path = "route_local_deps_graph.rs"]
mod graph;

#[tokio::test]
async fn issue_1330_review_escaped_import_dependency() {
    for source in [
        "package main\nimport _ \"example.org/\\x61pple\"\n",
        "package main\nimport (\n apple \"example.org/\\141pple/sub\"\n)\n",
        "package main\nimport _ \"example.org/\\u0061pple\"\n",
        "package main\nimport _ \"example.org/\\U00000061pple\"\n",
        "package main\nimport _ `example.org/a\rpple`\n",
    ] {
        let temp = fixture(SINGLE, source);
        assert_routes(temp.path(), Some("apple/apple.go")).await;
    }
}

#[tokio::test]
async fn issue_1330_review_raw_import_preserves_backslash() {
    let temp = fixture(SINGLE, "package main\nimport _ `example.org/\\x61pple`\n");
    assert_routes(temp.path(), None).await;
}

#[tokio::test]
async fn issue_1330_review_escaped_manifest_dependency() {
    for manifest in [
        "module example.org/root\nrequire \"example.org/\\u0061pple\" v1.0.0\nreplace \"example.org/\\x61pple\" => \"./\\141pple\"\n",
        "module example.org/root\nrequire (\n \"example.org/\\U00000061pple\" v1.0.0\n)\nreplace (\n example.org/apple => `./apple`\n)\n",
    ] {
        let temp = fixture(manifest, "package main\nimport _ \"example.org/apple\"\n");
        assert_routes(temp.path(), Some("apple/apple.go")).await;
    }
}

fn write(root: &std::path::Path, path: &str, content: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

// Real contained fixtures and public entry points; platforms/engines are simulated.
async fn assert_routes(root: &std::path::Path, constrained_file: Option<&str>) {
    let detected = crate::sync::macos_only_cgo(root);
    let linux = Node::new("linux x86_64", &["go"], false).await;
    let mac = Node::new("macos aarch64", &["go"], false).await;
    let nodes = [linux.addr, mac.addr];
    let expected = if constrained_file.is_some() {
        mac.addr
    } else {
        linux.addr
    };
    let checkout = checkout_node_in(&nodes, "dependency-review", linux.addr, root, None).await;
    let route = route_in(&nodes, "dependency-review", linux.addr, root, ".", None).await;
    let linux_only = route_in(
        &[linux.addr],
        "dependency-review",
        linux.addr,
        root,
        ".",
        None,
    )
    .await;
    linux.stop().await;
    mac.stop().await;
    let expected_detection = constrained_file.map(|p| (p.into(), "libproc.h".into()));
    assert_eq!(
        (
            detected,
            checkout.unwrap() == expected,
            route.unwrap() == expected,
            linux_only.is_ok(),
        ),
        (expected_detection, true, true, constrained_file.is_none(),),
        "detector, checkout, compatible-default routing, Linux-only routing"
    );
}

#[tokio::test]
async fn issue_1330_review_test_only_dependency() {
    for source in [
        "package main\nimport (\n _ \"example.org/apple\"\n)\n",
        "package main_test\nimport _ \"example.org/apple/sub\"\n",
    ] {
        let temp = fixture(SINGLE, "package main\n");
        write(temp.path(), "main_test.go", source);
        assert_routes(temp.path(), Some("apple/apple.go")).await;
    }
}

#[tokio::test]
async fn issue_1330_review_exact_replacement_precedence() {
    for replacements in [
        "replace example.org/apple => ./apple\nreplace example.org/apple v1.0.0 => ./portable\n",
        "replace (\n example.org/apple v1.0.0 => ./portable\n example.org/apple => ./apple\n)\n",
    ] {
        let manifest =
            format!("module example.org/root\nrequire example.org/apple v1.0.0\n{replacements}");
        let temp = fixture(&manifest, "package main\nimport _ \"example.org/apple\"\n");
        write(temp.path(), "portable/go.mod", "module example.org/apple\n");
        write(temp.path(), "portable/apple.go", "package apple\n");
        assert_routes(temp.path(), None).await;
    }
}

#[tokio::test]
async fn issue_1330_review_replacement_fallback_and_remote_override() {
    for (replacements, expected) in [
        (
            "replace (\n example.org/apple => ./portable\n example.org/apple v1.0.0 => ./apple\n)\n",
            Some("apple/apple.go"),
        ),
        (
            "replace (\n example.org/apple => ./portable\n example.org/apple v2.0.0 => ./apple\n)\n",
            None,
        ),
        (
            "replace (\n example.org/apple => ./apple\n example.org/apple v1.0.0 => example.org/portable v1.0.0\n)\n",
            None,
        ),
    ] {
        let manifest =
            format!("module example.org/root\nrequire example.org/apple v1.0.0\n{replacements}");
        let temp = fixture(&manifest, "package main\nimport _ \"example.org/apple\"\n");
        write(temp.path(), "portable/go.mod", "module example.org/apple\n");
        write(temp.path(), "portable/apple.go", "package apple\n");
        assert_routes(temp.path(), expected).await;
    }
}

#[tokio::test]
async fn issue_1330_review_inapplicable_tests_and_direct_cgo_excluded() {
    for (name, source) in [
        (
            "main_darwin_test.go",
            "package main\nimport _ \"example.org/apple\"\n",
        ),
        (
            "main_darwin_arm64_test.go",
            "package main\nimport _ \"example.org/apple\"\n",
        ),
        (
            "main_ios_test.go",
            "package main\nimport _ \"example.org/apple\"\n",
        ),
        (
            "main_test.go",
            "//go:build darwin\n\npackage main\nimport _ \"example.org/apple\"\n",
        ),
        // Go rejects direct cgo use in tests; it must not create an OS constraint here.
        ("main_test.go", APPLE),
    ] {
        let temp = fixture(SINGLE, "package main\n");
        write(temp.path(), name, source);
        assert_routes(temp.path(), None).await;
    }
}

#[tokio::test]
async fn issue_1330_review_dependency_imports_per_owner() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "a/go.mod",
        "module example.org/a\nrequire (\n example.org/shared v1.0.0\n example.org/apple v1.0.0\n)\nreplace (\n example.org/shared => ../zshared\n example.org/apple => ./apple\n)\n",
    );
    write(
        root,
        "a/main.go",
        "package a\nimport _ \"example.org/shared\"\n",
    );
    write(root, "a/apple/go.mod", "module example.org/apple\n");
    write(root, "a/apple/apple.go", APPLE);
    write(root, "zshared/go.mod", "module example.org/shared\n");
    write(
        root,
        "zshared/shared.go",
        "package shared\nimport _ \"example.org/apple\"\n",
    );
    assert_routes(root, Some("a/apple/apple.go")).await;
}

#[tokio::test]
async fn issue_1330_review_competing_required_versions() {
    // Numeric components, prereleases, pseudo-versions and build metadata use Go ordering.
    for (low, high) in [
        ("v1.9.0", "v1.10.0"),
        ("v1.0.0-rc.2", "v1.0.0-rc.10"),
        ("v1.0.0-2", "v1.0.0-beta"),
        ("v1.0.0-rc.10", "v1.0.0+incompatible"),
        (
            "v0.0.0-20260101000000-aaaaaaaaaaaa",
            "v0.0.0-20260201000000-bbbbbbbbbbbb",
        ),
    ] {
        for apple_is_high in [false, true] {
            for (main_version, bridge_version) in [(low, high), (high, low)] {
                let (lower, higher, expected) = if apple_is_high {
                    ("./portable", "./apple", Some("apple/apple.go"))
                } else {
                    ("./apple", "./portable", None)
                };
                let manifest = format!(
                    "module example.org/root\ngo 1.16\nrequire (\n example.org/apple {main_version}\n example.org/bridge v1.0.0\n)\nreplace (\n example.org/apple {low} => {lower}\n example.org/apple {high} => {higher}\n example.org/bridge => ./bridge\n)\n"
                );
                let temp = fixture(
                    &manifest,
                    "package main\nimport (\n _ \"example.org/apple\"\n _ \"example.org/bridge\"\n)\n",
                );
                write(temp.path(), "portable/go.mod", "module example.org/apple\n");
                write(temp.path(), "portable/apple.go", "package apple\n");
                write(
                    temp.path(),
                    "bridge/go.mod",
                    &format!(
                        "module example.org/bridge\nrequire example.org/apple {bridge_version}\n"
                    ),
                );
                write(
                    temp.path(),
                    "bridge/bridge.go",
                    "package bridge\nimport _ \"example.org/apple\"\n",
                );
                assert_routes(temp.path(), expected).await;
            }
        }
    }
}

#[tokio::test]
async fn issue_1330_review_manifest_closure_before_source_scan() {
    // Go 1.16 loads transitive requirements even when the bridging modules are not imported.
    let manifest = "module example.org/root\ngo 1.16\nrequire (\n example.org/apple v1.0.0\n example.org/bridge v1.0.0\n)\nreplace (\n example.org/apple v1.0.0 => ./apple\n example.org/apple v1.1.0 => ./portable\n example.org/bridge => ./bridge\n example.org/leaf => ./leaf\n)\n";
    let temp = fixture(manifest, "package main\nimport _ \"example.org/apple\"\n");
    write(temp.path(), "portable/go.mod", "module example.org/apple\n");
    write(temp.path(), "portable/apple.go", "package apple\n");
    write(
        temp.path(),
        "bridge/go.mod",
        "module example.org/bridge\nrequire example.org/leaf v1.0.0\n",
    );
    write(
        temp.path(),
        "leaf/go.mod",
        "module example.org/leaf\nrequire example.org/apple v1.1.0\n",
    );
    // Manifest reachability does not authorize source scanning without an import.
    write(temp.path(), "bridge/bridge.go", APPLE);
    write(temp.path(), "leaf/leaf.go", APPLE);
    assert_routes(temp.path(), None).await;
}

#[tokio::test]
async fn issue_1330_review_dependency_tests_do_not_expand_imports() {
    for test_path in ["bridge/bridge_test.go", "bridge/sub/sub_test.go"] {
        let manifest = "module example.org/root\nrequire (\n example.org/apple v1.0.0\n example.org/bridge v1.0.0\n)\nreplace (\n example.org/apple => ./apple\n example.org/bridge => ./bridge\n)\n";
        let temp = fixture(manifest, "package main\nimport _ \"example.org/bridge\"\n");
        write(temp.path(), "bridge/go.mod", "module example.org/bridge\n");
        write(temp.path(), "bridge/bridge.go", "package bridge\n");
        write(
            temp.path(),
            test_path,
            "package bridge_test\nimport _ \"example.org/apple\"\n",
        );
        assert_routes(temp.path(), None).await;
        // Ordinary dependency imports must still expand under the selected owner's table.
        write(
            temp.path(),
            "bridge/bridge.go",
            "package bridge\nimport _ \"example.org/apple\"\n",
        );
        assert_routes(temp.path(), Some("apple/apple.go")).await;
    }
}
