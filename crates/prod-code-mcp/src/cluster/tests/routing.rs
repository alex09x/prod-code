/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;

use prod_code_protocol::StatusResponse;

use super::helpers::node_serving;
use crate::cluster::parse::parse_remotes;
use crate::cluster::placement::{Placement, load_placement, save_placement};
use crate::cluster::routing::{checkout_node_in, nested_engine, route_for_path, route_in};
use crate::cluster::selection::{choose_quietest, rendezvous_order, supports_engine};

#[test]
fn parses_lists_and_dedups() {
    let nodes = parse_remotes("127.0.0.1:9400, 127.0.0.1:9401,127.0.0.1:9400").unwrap();
    assert_eq!(nodes.len(), 2);
    assert!(parse_remotes(" , ").is_err());
}

#[test]
fn parses_omitted_port_with_default() {
    let nodes = parse_remotes("127.0.0.1, localhost").unwrap();
    assert!(nodes.contains(&"127.0.0.1:9400".parse().unwrap()));
}

#[test]
fn parses_auto_discovery_seeds() {
    let nodes = parse_remotes("auto").unwrap();
    assert!(!nodes.is_empty());
    assert!(nodes.contains(&"127.0.0.1:9400".parse().unwrap()));
}

#[test]
fn rendezvous_is_stable_and_spreads() {
    let nodes = parse_remotes("10.0.0.1:9400,10.0.0.2:9400,10.0.0.3:9400").unwrap();
    let a = rendezvous_order(&nodes, "repo-a");
    assert_eq!(a, rendezvous_order(&nodes, "repo-a"));
    assert_eq!(a.len(), 3);
    // Removing a node keeps the relative order of the survivors.
    let fewer: Vec<SocketAddr> = nodes.iter().copied().filter(|n| *n != a[0]).collect();
    let b = rendezvous_order(&fewer, "repo-a");
    assert_eq!(
        b,
        a.into_iter()
            .filter(|n| fewer.contains(n))
            .collect::<Vec<_>>()
    );
    let homes: std::collections::HashSet<SocketAddr> = (0..64)
        .map(|i| rendezvous_order(&nodes, &format!("repo-{i}"))[0])
        .collect();
    assert_eq!(homes.len(), 3, "64 workspaces should land on all 3 nodes");
}

#[test]
fn quietest_prefers_low_load_then_rendezvous_order() {
    let a: SocketAddr = "10.0.0.1:9400".parse().unwrap();
    let b: SocketAddr = "10.0.0.2:9400".parse().unwrap();
    let c: SocketAddr = "10.0.0.3:9400".parse().unwrap();
    assert_eq!(
        choose_quietest(&[(a, Some(0.8)), (b, Some(0.1)), (c, None)]),
        Some(b)
    );
    assert_eq!(choose_quietest(&[(a, None), (b, None)]), Some(a));
    assert_eq!(choose_quietest(&[(a, Some(0.2)), (b, Some(0.2))]), Some(a));
    assert_eq!(choose_quietest(&[]), None);
}

/// A path in a SwiftPM package under a Rust repository needs the swift engine; a path of
/// the Rust project itself needs no other node (#125).
#[tokio::test]
async fn a_path_in_a_nested_project_names_its_engine() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"r\"\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "").unwrap();
    std::fs::create_dir_all(root.join("swift/Sources/App")).unwrap();
    std::fs::write(root.join("swift/Package.swift"), "").unwrap();
    std::fs::write(root.join("swift/Sources/App/main.swift"), "").unwrap();

    assert_eq!(nested_engine(&root, "swift"), Some("swift"));
    assert_eq!(
        nested_engine(&root, "swift/Sources/App/main.swift"),
        Some("swift")
    );
    let absolute = root.join("swift/Package.swift");
    assert_eq!(
        nested_engine(&root, &absolute.to_string_lossy()),
        Some("swift")
    );
    assert_eq!(nested_engine(&root, "src/lib.rs"), None);
    assert_eq!(nested_engine(&root, "."), None);

    // Without routing set up, a request goes where it was sent, whatever it names.
    let default: SocketAddr = "127.0.0.1:1".parse().unwrap();
    assert_eq!(
        route_for_path(default, &root, Some("swift")).await.unwrap(),
        default
    );
}

/// With a Rust node and a Swift node, a path in the SwiftPM package goes to the Swift node and
/// is remembered under its own key; a Rust path stays on the Rust node; with no Swift node the
/// error says what the path needs (#125).
#[tokio::test]
async fn a_swift_path_is_routed_to_the_node_that_serves_swift() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"r\"\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "").unwrap();
    std::fs::create_dir_all(root.join("swift/Sources/App")).unwrap();
    std::fs::write(root.join("swift/Package.swift"), "").unwrap();
    let placement = temp.path().join("placement.json");

    let rust = node_serving(&["rust", "go"]).await;
    let swift = node_serving(&["swift (sourcekit-lsp)"]).await;
    let nodes = [rust, swift];
    let routed = route_in(&nodes, "mixed", rust, &root, "swift", Some(&placement))
        .await
        .unwrap();
    assert_eq!(routed, swift);
    assert_eq!(
        load_placement(&placement).workspaces.get("mixed#swift"),
        Some(&swift)
    );
    assert!(!load_placement(&placement).workspaces.contains_key("mixed"));
    let stays = route_in(&nodes, "mixed", rust, &root, "src/lib.rs", Some(&placement))
        .await
        .unwrap();
    assert_eq!(stays, rust);
    // A default node that serves the engine keeps the call.
    let kept = route_in(&nodes, "mixed", swift, &root, "swift", Some(&placement))
        .await
        .unwrap();
    assert_eq!(kept, swift);

    let other_rust = node_serving(&["rust"]).await;
    let err = route_in(&[rust, other_rust], "mixed", rust, &root, "swift", None)
        .await
        .expect_err("no node serves swift");
    let text = format!("{err:#}");
    assert!(text.contains("is in a swift project"), "{text}");
    assert!(text.contains("serves swift"), "{text}");
}

/// Another checkout is asked on the node its own workspace is placed on, not on this one's;
/// this checkout itself stays on the node it was given (#375).
#[tokio::test]
async fn another_checkout_is_routed_to_its_own_node() {
    let temp = tempfile::tempdir().unwrap();
    let other = std::fs::canonicalize(temp.path()).unwrap().join("other");
    std::fs::create_dir_all(other.join("src")).unwrap();
    std::fs::write(other.join("Cargo.toml"), "[package]\nname = \"o\"\n").unwrap();
    std::fs::write(other.join("src/lib.rs"), "").unwrap();
    let identity = crate::sync::workspace_identity(&other);
    let name = identity.base.unwrap_or(identity.name);
    let placement = temp.path().join("placement.json");
    let here = node_serving(&["rust"]).await;
    let there = node_serving(&["rust"]).await;
    let mut remembered = Placement::default();
    remembered.workspaces.insert(name.clone(), there);
    save_placement(&placement, &remembered);

    let nodes = [here, there];
    let routed = checkout_node_in(&nodes, "this", here, &other, Some(&placement))
        .await
        .unwrap();
    assert_eq!(routed, there);
    let itself = checkout_node_in(&nodes, &name, here, &other, Some(&placement))
        .await
        .unwrap();
    assert_eq!(itself, here);
}

#[test]
fn engine_support_matches_labelled_entries() {
    let status = StatusResponse {
        server_pid: 1,
        uptime_seconds: 0,
        active_sessions: 0,
        loaded_workspaces: 0,
        detected_engines: vec![
            "rust (ra_ap_ide)".to_string(),
            "swift (sourcekit-lsp)".to_string(),
            "generic-lsp".to_string(),
        ],
        memory_rss_bytes: None,
        total_queries: 0,
        active_queries: 0,
        load_average_millis: None,
        cpu_count: None,
        platform: None,
        running_commands: Vec::new(),
        host: Default::default(),
        version: None,
        git_commit: None,
    };
    assert!(supports_engine(&status, "rust"));
    assert!(supports_engine(&status, "swift"));
    assert!(supports_engine(&status, "generic-lsp"));
    assert!(!supports_engine(&status, "go"));
    assert!(!supports_engine(&status, "swif"));
}
