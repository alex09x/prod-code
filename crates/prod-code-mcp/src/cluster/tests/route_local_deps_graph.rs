/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{APPLE, assert_routes, fixture, write};

fn version_graph(main_go: &str, bridge_go: &str, upgraded_apple: bool) -> tempfile::TempDir {
    let (older, newer) = if upgraded_apple {
        ("portable", "apple")
    } else {
        ("apple", "portable")
    };
    let manifest = format!(
        "module example.org/root\ngo {main_go}\nrequire (\n example.org/apple v1.0.0\n example.org/shared v1.1.0\n example.org/bridge v1.0.0\n)\nreplace (\n example.org/apple v1.0.0 => ./{older}\n example.org/apple v1.1.0 => ./{newer}\n example.org/shared v1.0.0 => ./shared-low\n example.org/shared v1.1.0 => ./shared-high\n example.org/bridge => ./bridge\n)\n"
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
        &format!("module example.org/bridge\ngo {bridge_go}\nrequire example.org/shared v1.0.0\n"),
    );
    write(
        root,
        "shared-high/go.mod",
        "module example.org/shared\ngo 1.24\n",
    );
    write(
        root,
        "shared-low/go.mod",
        "module example.org/shared\ngo 1.24\nrequire example.org/apple v1.1.0\n",
    );
    // These modules contribute graph edges, but are not imported source dependencies.
    for path in [
        "bridge/bridge.go",
        "shared-high/shared.go",
        "shared-low/shared.go",
    ] {
        write(root, path, APPLE);
    }
    temp
}

#[tokio::test]
async fn issue_1330_graph_lower_required_version_contributes_edges() {
    // Both an old main module and an old dependency retain the full requirement closure.
    // shared v1.0.0 is not selected, but its edge upgrades apple to v1.1.0.
    for (main_go, bridge_go) in [("1.16", "1.24"), ("1.24", "1.16")] {
        let temp = version_graph(main_go, bridge_go, true);
        assert_routes(temp.path(), Some("apple/apple.go")).await;
    }
}

#[tokio::test]
async fn issue_1330_graph_source_uses_selected_version_only() {
    for (main_go, bridge_go) in [("1.16", "1.24"), ("1.24", "1.16")] {
        // Inverting replacement sources must select the portable upgraded version, rather
        // than scanning either the discarded Apple version or all graph manifests' sources.
        let temp = version_graph(main_go, bridge_go, false);
        assert_routes(temp.path(), None).await;
    }
}

#[tokio::test]
async fn issue_1330_graph_modern_pruning_excludes_lower_nonroot_edges() {
    // A modern bridge's requirement on an unselected shared version is not an unpruned edge.
    for upgraded_apple in [true, false] {
        let temp = version_graph("1.24", "1.24", upgraded_apple);
        let expected = (!upgraded_apple).then_some("apple/apple.go");
        assert_routes(temp.path(), expected).await;
    }
}
