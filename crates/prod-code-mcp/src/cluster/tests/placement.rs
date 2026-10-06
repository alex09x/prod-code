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

use super::helpers::{node_on, node_with};
use crate::cluster::pick::pick_node_with;
use crate::cluster::placement::{Placement, load_placement, save_placement};

/// Without a cluster answer, a node short of memory or disk is passed over for a busier one
/// that has room, and taken only when every node is short (#396).
#[tokio::test]
async fn a_node_short_of_disk_is_passed_over_without_a_cluster_view() {
    let temp = tempfile::tempdir().unwrap();
    let placement = temp.path().join("placement.json");
    let full_disk = HostResources {
        storage_free_millis: Some(20),
        ..HostResources::default()
    };
    let roomy = HostResources {
        storage_free_millis: Some(600),
        ..HostResources::default()
    };
    let quiet_but_full = node_with(&["rust"], Some("linux x86_64"), 40, full_disk.clone()).await;
    let busier = node_with(&["rust"], Some("linux x86_64"), 2000, roomy).await;
    let chosen = pick_node_with(
        &[quiet_but_full, busier],
        "subject",
        Some("rust"),
        None,
        Some(&placement),
    )
    .await
    .unwrap();
    assert_eq!(chosen, busier);

    let also_full = node_with(&["rust"], Some("linux x86_64"), 2000, full_disk).await;
    let chosen = pick_node_with(
        &[quiet_but_full, also_full],
        "other",
        Some("rust"),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(chosen, quiet_but_full, "every node is short: the quietest");
}

/// A checkout that needs macOS leaves a remembered Linux node for the macOS one, and a node
/// too old to report its platform does not count as macOS. Without a macOS node the error
/// says so, with one node or several; a checkout that needs no OS still takes Linux (#248).
#[tokio::test]
async fn a_checkout_that_needs_macos_is_placed_on_a_macos_node() {
    let temp = tempfile::tempdir().unwrap();
    let placement = temp.path().join("placement.json");
    let linux = node_on(&["go"], Some("linux x86_64")).await;
    let old = node_on(&["go"], None).await;
    let mac = node_on(&["go", "swift (sourcekit-lsp)"], Some("macos aarch64")).await;
    let mut remembered = Placement::default();
    remembered.workspaces.insert("cgo".to_string(), linux);
    save_placement(&placement, &remembered);

    let nodes = [linux, old, mac];
    let picked = pick_node_with(&nodes, "cgo", Some("go"), Some("macos"), Some(&placement))
        .await
        .unwrap();
    assert_eq!(picked, mac);
    assert_eq!(load_placement(&placement).workspaces.get("cgo"), Some(&mac));
    let anywhere = pick_node_with(&[linux, old], "plain", Some("go"), None, None)
        .await
        .unwrap();
    assert!(anywhere == linux || anywhere == old, "{anywhere}");

    let err = pick_node_with(&[linux, old], "cgo", Some("go"), Some("macos"), None)
        .await
        .expect_err("no node runs macOS");
    let text = format!("{err:#}");
    assert!(text.contains("no reachable gateway runs macOS"), "{text}");
    assert!(text.contains(&linux.to_string()), "{text}");
    assert!(text.contains(&old.to_string()), "{text}");
    let err = pick_node_with(&[linux], "cgo", Some("go"), Some("macos"), None)
        .await
        .expect_err("the one node runs Linux");
    assert!(
        format!("{err:#}").contains("no reachable gateway runs macOS"),
        "{err:#}"
    );
    assert_eq!(
        pick_node_with(&[mac], "cgo", Some("go"), Some("macos"), None)
            .await
            .unwrap(),
        mac
    );
}

/// Without a cluster answer, work that does not need macOS still keeps off a macOS node
/// while another node serves it, whatever the rendezvous order says, and takes the Mac only
/// when nothing else can (#308).
#[tokio::test]
async fn plain_work_keeps_off_a_macos_node_while_another_node_serves_it() {
    let linux = node_on(&["go"], Some("linux x86_64")).await;
    let mac = node_on(&["go", "swift (sourcekit-lsp)"], Some("macos aarch64")).await;
    for i in 0..20 {
        let name = format!("plain-{i}");
        let picked = pick_node_with(&[mac, linux], &name, Some("go"), None, None)
            .await
            .unwrap();
        assert_eq!(picked, linux, "{name} went to the Mac");
    }
    assert_eq!(
        pick_node_with(&[mac, linux], "app", Some("swift"), None, None)
            .await
            .unwrap(),
        mac,
        "only the Mac serves Swift"
    );
    assert_eq!(
        pick_node_with(&[mac], "plain", Some("go"), None, None)
            .await
            .unwrap(),
        mac,
        "alone, the Mac takes plain Go"
    );
}

/// A remembered node that is down for a moment, as a gateway is while it restarts, keeps the
/// workspace; one that stays down loses it to a live node (#238).
#[tokio::test]
async fn a_remembered_node_that_restarts_keeps_the_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let placement = temp.path().join("placement.json");
    let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let other_addr = other.local_addr().unwrap();
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let home = closed.local_addr().unwrap();
    drop(closed);
    let mut remembered = Placement::default();
    remembered.workspaces.insert("ws".to_string(), home);
    save_placement(&placement, &remembered);
    let revived = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
        let listener = tokio::net::TcpListener::bind(home).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        drop(listener);
    });
    let picked = pick_node_with(&[other_addr, home], "ws", None, None, Some(&placement))
        .await
        .unwrap();
    assert_eq!(picked, home, "the restarted node keeps the workspace");
    revived.abort();

    // Down for good: after the retries the workspace moves to the live node.
    let gone = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gone_addr = gone.local_addr().unwrap();
    drop(gone);
    let mut remembered = Placement::default();
    remembered.workspaces.insert("ws2".to_string(), gone_addr);
    save_placement(&placement, &remembered);
    let picked = pick_node_with(
        &[other_addr, gone_addr],
        "ws2",
        None,
        None,
        Some(&placement),
    )
    .await
    .unwrap();
    assert_eq!(picked, other_addr);
    drop(other);
}

#[tokio::test]
async fn picks_alive_node_and_remembers_it() {
    let temp = tempfile::tempdir().unwrap();
    let placement = temp.path().join("placement.json");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let alive = listener.local_addr().unwrap();
    let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let nodes = vec![dead, alive];
    let picked = pick_node_with(&nodes, "ws", None, None, Some(&placement))
        .await
        .unwrap();
    assert_eq!(picked, alive);
    let saved = load_placement(&placement);
    assert_eq!(saved.workspaces.get("ws"), Some(&alive));
    // A single node is used without probing.
    assert_eq!(
        pick_node_with(&[dead], "ws", None, None, None)
            .await
            .unwrap(),
        dead
    );
    assert!(pick_node_with(&[dead], "", None, None, None).await.is_ok());
    assert!(pick_node_with(&[], "ws", None, None, None).await.is_err());
}

/// When no node answers, the error says why the first did not (#340).
#[tokio::test]
async fn no_node_reachable_names_why_the_first_did_not_answer() {
    let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let also_dead: SocketAddr = "127.0.0.1:2".parse().unwrap();
    let err = pick_node_with(&[dead, also_dead], "ws", Some("rust"), None, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.starts_with("no gateway reachable among"), "{err}");
    assert!(
        err.contains("127.0.0.1:") && err.to_lowercase().contains("refused"),
        "the operating system's reason is named: {err}"
    );
    assert!(err.contains("ssh -NL 9400:localhost:9400"), "{err}");
}

#[tokio::test]
async fn retains_remembered_placement_when_single_seed_unsupported_or_down() {
    let temp = tempfile::tempdir().unwrap();
    let placement = temp.path().join("placement.json");
    let mac = node_on(&["swift (sourcekit-lsp)"], Some("macos aarch64")).await;
    let mut remembered = Placement::default();
    remembered.workspaces.insert("ws#swift".to_string(), mac);
    save_placement(&placement, &remembered);

    let dead_loopback: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let picked = pick_node_with(
        &[dead_loopback],
        "ws#swift",
        Some("swift"),
        None,
        Some(&placement),
    )
    .await
    .unwrap();
    assert_eq!(
        picked, mac,
        "retains remembered mac node for swift workspace"
    );
}
