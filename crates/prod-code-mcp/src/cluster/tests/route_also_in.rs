/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A nested project of another checkout, asked about through `also_in`, is placed under that
//! checkout's name, not under the checkout this process started in (#1424).

use super::Node;
use crate::cluster::placement::load_placement;
use crate::cluster::routing::{Routing, placement_workspace, route_with};

fn checkout(prefix: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
    for (rel, text) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

#[tokio::test]
async fn a_nested_project_of_another_checkout_is_placed_under_that_checkout() {
    let origin = checkout("origin-a", &[("Cargo.toml", "[package]\nname = \"a\"\n")]);
    let other = checkout(
        "other-b",
        &[
            ("Cargo.toml", "[package]\nname = \"b\"\n"),
            ("tools/util/go.mod", "module example.org/util\n"),
            ("tools/util/main.go", "package main\n\nfunc main() {}\n"),
        ],
    );
    let rust = Node::new("linux x86_64", &["rust"], false).await;
    let go = Node::new("linux x86_64", &["go"], false).await;
    let routing = Routing {
        nodes: vec![rust.addr, go.addr],
        workspace: placement_workspace(origin.path()),
    };
    let state = tempfile::tempdir().unwrap();
    let placement = state.path().join("placement.json");

    let chosen = route_with(
        &routing,
        rust.addr,
        other.path(),
        "tools/util/main.go",
        Some(&placement),
    )
    .await
    .unwrap();
    assert_eq!(chosen, go.addr, "the Rust default cannot serve Go");
    let keys: Vec<String> = load_placement(&placement).workspaces.into_keys().collect();
    let other_name = placement_workspace(other.path());
    assert!(keys.contains(&format!("{other_name}#go")), "{keys:?}");
    assert!(
        !keys.iter().any(|k| k.starts_with(&routing.workspace)),
        "nothing is remembered under the origin checkout: {keys:?}"
    );
    rust.stop().await;
    go.stop().await;
}
