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

use prod_code_protocol::HostResources;

use super::helpers::{node_on, node_with, seed_node_with_peers};
use crate::cluster::discover::discover_nodes_with_paths;
use crate::cluster::pick::pick_node_with;
use crate::cluster::placement::{Placement, load_placement, save_placement};
use crate::cluster::rebalance::evaluate_cluster_rebalance_with;
use crate::cluster::selection::{choose_best_node, is_alive};

#[tokio::test]
async fn discover_nodes_preserves_remembered_placement_when_seed_view_omits_it() {
    let temp = tempfile::tempdir().unwrap();
    let placement = temp.path().join("placement.json");
    let cache = temp.path().join("cluster.json");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let seed_addr = listener.local_addr().unwrap();
    drop(listener);
    let seed = seed_node_with_peers(vec![seed_addr]).await;

    let mac = node_on(&["swift (sourcekit-lsp)"], Some("macos aarch64")).await;
    let mut rem = Placement::default();
    rem.workspaces.insert("ws#swift".to_string(), mac);
    save_placement(&placement, &rem);

    let discovered = discover_nodes_with_paths(&[seed], Some(&placement), Some(&cache)).await;

    assert!(
        discovered.contains(&mac),
        "discovered nodes must include remembered placement {mac}: {discovered:?}"
    );
    assert!(
        discovered.contains(&seed),
        "discovered nodes must include seed {seed}: {discovered:?}"
    );

    let picked = pick_node_with(
        &discovered,
        "ws#swift",
        Some("swift"),
        None,
        Some(&placement),
    )
    .await
    .unwrap();
    assert_eq!(
        picked, mac,
        "pick_node_with must choose remembered mac node"
    );
}

#[tokio::test]
async fn explicit_remote_does_not_fall_back_to_remembered_placement() {
    let temp = tempfile::tempdir().unwrap();
    let placement = temp.path().join("placement.json");

    let remembered = node_on(&["rust"], Some("linux x86_64")).await;
    let mut rem = Placement::default();
    rem.workspaces.insert("subject".to_string(), remembered);
    save_placement(&placement, &rem);

    let explicit_remote: SocketAddr = "192.0.2.168:9400".parse().unwrap();
    let picked = pick_node_with(
        &[explicit_remote],
        "subject",
        Some("rust"),
        None,
        Some(&placement),
    )
    .await
    .unwrap();

    assert_eq!(picked, explicit_remote);
}

#[tokio::test]
async fn default_loopback_remote_falls_back_to_remembered_placement_when_dead() {
    let temp = tempfile::tempdir().unwrap();
    let placement = temp.path().join("placement.json");

    let remembered = node_on(&["rust"], Some("linux x86_64")).await;
    let mut rem = Placement::default();
    rem.workspaces.insert("subject".to_string(), remembered);
    save_placement(&placement, &rem);

    let default_loopback: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    if !is_alive(default_loopback).await {
        let picked = pick_node_with(
            &[default_loopback],
            "subject",
            Some("rust"),
            None,
            Some(&placement),
        )
        .await
        .unwrap();
        assert_eq!(picked, remembered);
    }
}

#[test]
fn choose_best_node_picks_lowest_congestion_score() {
    let node1: SocketAddr = "10.0.0.1:9400".parse().unwrap();
    let node2: SocketAddr = "10.0.0.2:9400".parse().unwrap();
    let node3: SocketAddr = "10.0.0.3:9400".parse().unwrap();

    let candidates = vec![(node1, 1.45), (node2, 0.05), (node3, 0.82)];

    assert_eq!(choose_best_node(&candidates), Some(node2));
}

#[tokio::test]
async fn pick_node_with_rebalances_when_remembered_node_is_congested() {
    let temp = tempfile::tempdir().unwrap();
    let placement = temp.path().join("placement.json");

    let roomy_disk = HostResources {
        storage_free_millis: Some(600),
        ..HostResources::default()
    };

    // Congested node: 4.8 load / 4 cpus = 1.2 load/cpu, score ~1.2
    let congested = node_with(&["rust"], Some("linux x86_64"), 4800, roomy_disk.clone()).await;
    // Roomy node: 0.08 load / 4 cpus = 0.02 load/cpu, score ~0.02
    let roomy = node_with(&["rust"], Some("linux x86_64"), 80, roomy_disk).await;

    let mut rem = Placement::default();
    rem.workspaces.insert("subject".to_string(), congested);
    save_placement(&placement, &rem);

    // Before picking, placement points to `congested`
    assert_eq!(
        load_placement(&placement).workspaces.get("subject"),
        Some(&congested)
    );

    // When picking, it detects that `congested` has score >= 0.80 and `roomy` is >2x better,
    // so it rebalances to `roomy` and updates placement.json!
    let chosen = pick_node_with(
        &[congested, roomy],
        "subject",
        Some("rust"),
        None,
        Some(&placement),
    )
    .await
    .unwrap();

    assert_eq!(chosen, roomy);
    assert_eq!(
        load_placement(&placement).workspaces.get("subject"),
        Some(&roomy)
    );
}

#[tokio::test]
async fn unreachable_node_failover_avoids_macos_when_linux_peer_available() {
    let temp = tempfile::tempdir().unwrap();
    let placement = temp.path().join("placement.json");

    // Dead node (unbound port)
    let dead: SocketAddr = "127.0.0.1:40999".parse().unwrap();

    // macOS node: very quiet (load 0)
    let mac_node = node_with(
        &["rust", "swift"],
        Some("macos aarch64"),
        0,
        HostResources::default(),
    )
    .await;

    // Linux node: moderate load
    let linux_node = node_with(
        &["rust"],
        Some("linux x86_64"),
        100,
        HostResources::default(),
    )
    .await;

    let mut rem = Placement::default();
    rem.workspaces.insert("generic-work".to_string(), dead);
    save_placement(&placement, &rem);

    // When dead node is evaluated for generic work (os = None), it must pick the Linux node,
    // despite the macOS node having lower load.
    let rebalance = evaluate_cluster_rebalance_with(
        &[dead, mac_node, linux_node],
        dead,
        "generic-work",
        Some("rust"),
        None,
    )
    .await;

    assert_eq!(rebalance.map(|(addr, _)| addr), Some(linux_node));
}
