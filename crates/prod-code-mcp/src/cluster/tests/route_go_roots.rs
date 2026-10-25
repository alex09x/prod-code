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

async fn go_root(workspace: bool, root_module: bool) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("apple")).unwrap();
    std::fs::write(root.join("apple/go.mod"), "module example.org/apple\n").unwrap();
    std::fs::write(
        root.join("apple/main.go"),
        "package main\n/*\n#include <libproc.h>\n*/\nimport \"C\"\n",
    )
    .unwrap();
    if workspace {
        std::fs::write(
            root.join("go.work"),
            if root_module {
                "go 1.24\nuse (\n .\n ./apple\n)\n"
            } else {
                "go 1.24\nuse ./apple\n"
            },
        )
        .unwrap();
    }
    if root_module {
        std::fs::write(root.join("go.mod"), "module example.org/root\n").unwrap();
    }
    assert_eq!(crate::sync::expected_engine(root), Some("go"));
    let linux = Node::new("linux x86_64", &["rust", "go"], false).await;
    let mac = Node::new("macos aarch64", &["go"], false).await;
    let rust = Node::new("linux x86_64", &["rust"], false).await;
    let expected = mac.addr;
    let nodes = [linux.addr, mac.addr, rust.addr];
    let checkout = checkout_node_in(&nodes, "other-checkout", linux.addr, root, None).await;
    // Finish the fake endpoint tasks before checking the routing results.
    let mut routes = Vec::new();
    for default in [linux.addr, rust.addr] {
        for path in [".", "apple", "apple/main.go"] {
            routes.push((
                path,
                default,
                route_in(&nodes, "go-root", default, root, path, None).await,
            ));
        }
    }
    linux.stop().await;
    mac.stop().await;
    rust.stop().await;
    assert_eq!(
        checkout.unwrap(),
        expected,
        "workspace={workspace}, root_module={root_module}"
    );
    for (path, default, result) in routes {
        assert_eq!(
            result.unwrap(),
            expected,
            "path={path}, default={default}, workspace={workspace}, root_module={root_module}"
        );
    }
    assert_eq!(
        crate::sync::macos_only_cgo(root),
        Some(("apple/main.go".into(), "libproc.h".into()))
    );
}

#[tokio::test]
async fn issue_1330_workspace_root_keeps_member_cgo_requirement() {
    go_root(true, false).await;
}

#[tokio::test]
async fn issue_1330_workspace_with_root_module_keeps_member_cgo_requirement() {
    go_root(true, true).await;
}

#[tokio::test]
async fn issue_1330_wrapper_root_keeps_inferred_child_cgo_requirement() {
    go_root(false, false).await;
}

#[tokio::test]
async fn issue_1330_workspace_keeps_explicit_nested_member() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("plain/apple")).unwrap();
    for (path, source) in [
        ("go.work", "go 1.24\nuse (\n ./plain\n ./plain/apple\n)\n"),
        ("plain/go.mod", "module example.org/plain\n"),
        ("plain/main.go", "package main\n"),
        ("plain/apple/go.mod", "module example.org/apple\n"),
        (
            "plain/apple/main.go",
            "package main\n/*\n#include <libproc.h>\n*/\nimport \"C\"\n",
        ),
    ] {
        std::fs::write(root.join(path), source).unwrap();
    }
    assert_eq!(
        crate::sync::macos_only_cgo(root),
        Some(("plain/apple/main.go".into(), "libproc.h".into()))
    );
    let linux = Node::new("linux x86_64", &["go"], false).await;
    let mac = Node::new("macos aarch64", &["go"], false).await;
    let nodes = [linux.addr, mac.addr];
    let checkout = checkout_node_in(&nodes, "other-checkout", linux.addr, root, None).await;
    let route = route_in(&nodes, "workspace", linux.addr, root, ".", None).await;
    let expected = mac.addr;
    linux.stop().await;
    mac.stop().await;
    assert_eq!(checkout.unwrap(), expected);
    assert_eq!(route.unwrap(), expected);
}

#[tokio::test]
async fn issue_1330_wrapper_stops_at_selected_child_module_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("plain/apple")).unwrap();
    for (path, source) in [
        ("plain/go.mod", "module example.org/plain\n"),
        ("plain/main.go", "package main\n"),
        ("plain/apple/go.mod", "module example.org/apple\n"),
        (
            "plain/apple/main.go",
            "package main\n/*\n#include <libproc.h>\n*/\nimport \"C\"\n",
        ),
    ] {
        std::fs::write(root.join(path), source).unwrap();
    }
    assert_eq!(crate::sync::expected_engine(root), Some("go"));
    assert_eq!(crate::sync::macos_only_cgo(root), None);
    let linux = Node::new("linux x86_64", &["go"], false).await;
    let mac = Node::new("macos aarch64", &["go"], false).await;
    let nodes = [linux.addr, mac.addr];
    let checkout = checkout_node_in(&nodes, "other-checkout", mac.addr, root, None).await;
    let parent = route_in(&nodes, "wrapper", linux.addr, root, ".", None).await;
    let child = route_in(&nodes, "wrapper", linux.addr, root, "plain/apple", None).await;
    let expected_linux = linux.addr;
    let expected_mac = mac.addr;
    linux.stop().await;
    mac.stop().await;
    assert_eq!(checkout.unwrap(), expected_linux);
    assert_eq!(parent.unwrap(), expected_linux);
    assert_eq!(child.unwrap(), expected_mac);
}
