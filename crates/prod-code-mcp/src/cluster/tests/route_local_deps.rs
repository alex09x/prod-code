/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::Node;
use crate::cluster::{checkout_node_in, route_in};

#[path = "route_local_deps_review.rs"]
mod review;

const APPLE: &str = "package apple\n/*\n#include <libproc.h>\n*/\nimport \"C\"\nconst Value = 1\n";

fn fixture(manifest: &str, source: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    for (path, content) in [
        ("go.mod", manifest),
        ("main.go", source),
        ("apple/go.mod", "module example.org/apple\n"),
        ("apple/apple.go", APPLE),
        (
            "apple/sub/sub.go",
            "package sub\nimport _ \"example.org/apple\"\n",
        ),
        ("independent/go.mod", "module example.org/independent\n"),
        ("independent/apple.go", APPLE),
    ] {
        let path = temp.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    temp
}

const SINGLE: &str = "module example.org/root\nrequire example.org/apple v1.0.0\nreplace example.org/apple => ./apple\n";
const BLOCK: &str = "module example.org/root\nrequire (\n example.org/apple v1.0.0 // local\n)\nreplace (\n example.org/apple v1.0.0 => \"./apple\"\n)\n";

#[test]
fn issue_1330_local_dependency_detector() {
    for (manifest, source) in [
        (SINGLE, "package main\nimport _ \"example.org/apple\"\n"),
        (
            BLOCK,
            "package main\nimport (\n _ `example.org/apple/sub`\n)\n",
        ),
        (
            BLOCK,
            "// 中文 comment\npackage main\nimport apple \"example.org/apple\"\nvar _ = apple.Value\n",
        ),
    ] {
        let temp = fixture(manifest, source);
        assert_eq!(
            crate::sync::macos_only_cgo(temp.path()),
            Some(("apple/apple.go".into(), "libproc.h".into())),
            "required, replaced and imported child must retain its OS constraint"
        );
    }
}

#[tokio::test]
async fn issue_1330_local_dependency_public_routes() {
    let temp = fixture(SINGLE, "package main\nimport _ \"example.org/apple\"\n");
    let root = temp.path();
    let linux = Node::new("linux x86_64", &["go"], false).await;
    let mac = Node::new("macos aarch64", &["go"], false).await;
    let rust = Node::new("linux x86_64", &["rust"], false).await;
    let nodes = [linux.addr, mac.addr, rust.addr];
    let checkout = checkout_node_in(&nodes, "different", linux.addr, root, None).await;
    let compatible = route_in(&nodes, "local-deps", linux.addr, root, ".", None).await;
    let fallback = route_in(&nodes, "local-deps", rust.addr, root, "main.go", None).await;
    let missing = route_in(&[linux.addr], "local-deps", linux.addr, root, ".", None).await;
    let expected = mac.addr;
    linux.stop().await;
    mac.stop().await;
    rust.stop().await;
    assert_eq!(
        checkout.unwrap(),
        expected,
        "checkout lost local dependency OS"
    );
    assert_eq!(
        compatible.unwrap(),
        expected,
        "compatible default lost dependency OS"
    );
    assert_eq!(fallback.unwrap(), expected, "fallback lost dependency OS");
    assert!(format!("{:#}", missing.unwrap_err()).contains("macOS"));
}

#[tokio::test]
async fn issue_1330_local_dependency_unrelated_children_stay_excluded() {
    for (manifest, source) in [
        (SINGLE, "package main\n"),
        (SINGLE, "package main\nimport _ \"example.org/apples\"\n"),
        (SINGLE, "package main\n// import _ \"example.org/apple\"\n"),
        (
            SINGLE,
            "package main\n/*\nimport _ \"example.org/apple\"\n*/\n",
        ),
        (
            SINGLE,
            "//go:build darwin\n\npackage main\nimport _ \"example.org/apple\"\n",
        ),
        (
            SINGLE,
            "package main\nvar example = `\nimport _ \"example.org/apple\"\n`\n",
        ),
        (
            "module example.org/root\nreplace example.org/apple => ./apple\n",
            "package main\nimport _ \"example.org/apple\"\n",
        ),
        (
            "module example.org/root\nrequire example.org/apple v1.0.0\nreplace example.org/apple v2.0.0 => ./apple\n",
            "package main\nimport _ \"example.org/apple\"\n",
        ),
    ] {
        let temp = fixture(manifest, source);
        assert_eq!(crate::sync::macos_only_cgo(temp.path()), None);
        let linux = Node::new("linux x86_64", &["go"], false).await;
        let result = route_in(
            &[linux.addr],
            "portable",
            linux.addr,
            temp.path(),
            ".",
            None,
        )
        .await;
        let expected = linux.addr;
        linux.stop().await;
        assert_eq!(result.unwrap(), expected);
    }
}

#[test]
fn issue_1330_local_dependency_transitive_imports_and_cycles() {
    let temp = fixture(
        "module example.org/root\nrequire example.org/bridge v1.0.0\nreplace (\n example.org/bridge => ./bridge\n example.org/apple => ./apple\n)\n",
        "package main\nimport _ \"example.org/bridge\"\n",
    );
    std::fs::create_dir(temp.path().join("bridge")).unwrap();
    std::fs::write(temp.path().join("bridge/go.mod"), "module example.org/bridge\nrequire example.org/apple v1.0.0\nreplace example.org/apple => ../independent\n").unwrap();
    std::fs::write(
        temp.path().join("bridge/bridge.go"),
        "package bridge\nimport _ \"example.org/apple\"\n",
    )
    .unwrap();
    // A module requirement cycle does not require a package import cycle.
    std::fs::write(
        temp.path().join("apple/go.mod"),
        "module example.org/apple\nrequire example.org/bridge v1.0.0\n",
    )
    .unwrap();
    assert_eq!(
        crate::sync::macos_only_cgo(temp.path()),
        Some(("apple/apple.go".into(), "libproc.h".into()))
    );
}

#[test]
fn issue_1330_local_dependency_longest_import_owner() {
    let temp = fixture(
        "module example.org/root\nrequire (\n example.org/apple v1.0.0\n example.org/apple/portable v1.0.0\n)\nreplace (\n example.org/apple => ./apple\n example.org/apple/portable => ./independent\n)\n",
        "package main\nimport _ \"example.org/apple/portable\"\n",
    );
    std::fs::write(
        temp.path().join("independent/go.mod"),
        "module example.org/apple/portable\n",
    )
    .unwrap();
    std::fs::write(
        temp.path().join("independent/apple.go"),
        "package portable\n",
    )
    .unwrap();
    assert_eq!(crate::sync::macos_only_cgo(temp.path()), None);
}

#[test]
fn issue_1330_local_dependency_contained_sibling_and_external_escape() {
    let temp = fixture(SINGLE, "package main\nimport _ \"example.org/apple\"\n");
    let root = temp.path();
    std::fs::create_dir(root.join("parent")).unwrap();
    std::fs::rename(root.join("go.mod"), root.join("parent/go.mod")).unwrap();
    std::fs::rename(root.join("main.go"), root.join("parent/main.go")).unwrap();
    std::fs::write(
        root.join("parent/go.mod"),
        SINGLE.replace("./apple", "../apple"),
    )
    .unwrap();
    // The wrapper is the containment boundary, so its selected child can reach a sibling.
    assert_eq!(
        crate::sync::macos_only_cgo(root),
        Some(("apple/apple.go".into(), "libproc.h".into()))
    );
    // Selecting just parent must never inspect outside the provided scan boundary.
    assert_eq!(crate::sync::macos_only_cgo(&root.join("parent")), None);
}

#[cfg(unix)]
#[test]
fn issue_1330_local_dependency_symlink_escape_is_excluded() {
    let temp = fixture(SINGLE, "package main\nimport _ \"example.org/apple\"\n");
    let root = temp.path();
    std::fs::create_dir(root.join("parent")).unwrap();
    std::fs::rename(root.join("go.mod"), root.join("parent/go.mod")).unwrap();
    std::fs::rename(root.join("main.go"), root.join("parent/main.go")).unwrap();
    std::os::unix::fs::symlink(root.join("apple"), root.join("parent/apple")).unwrap();
    assert_eq!(crate::sync::macos_only_cgo(&root.join("parent")), None);
}
