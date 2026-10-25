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
use crate::cluster::placement::{Placement, load_placement, save_placement};
use crate::cluster::{pick_node_with, rendezvous_order};

fn mac_first(nodes: &[std::net::SocketAddr], mac: std::net::SocketAddr) -> String {
    (0..)
        .map(|i| format!("plain-{i}"))
        .find(|key| rendezvous_order(nodes, key)[0] == mac)
        .unwrap()
}

#[tokio::test]
async fn issue_1330_remembered_mac_moves_when_linux_returns() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("placement.json");
    let mac = Node::new("macos aarch64", &["go"], true).await;
    let linux = Node::new("linux x86_64", &["python"], false).await;
    let nodes = [mac.addr, linux.addr];
    let key = mac_first(&nodes, mac.addr);
    let mut placement = Placement::default();
    placement.workspaces.insert(key.clone(), mac.addr);
    save_placement(&path, &placement);
    assert_eq!(
        pick_node_with(&nodes, &key, Some("go"), None, Some(&path))
            .await
            .unwrap(),
        mac.addr
    );
    assert!(mac.requests.lock().unwrap().is_empty());
    // The mutex is a barrier: every subsequent probe sees the returned Go engine.
    linux.state.lock().unwrap().detected_engines = vec!["go".into()];
    let chosen = pick_node_with(&nodes, &key, Some("go"), None, Some(&path))
        .await
        .unwrap();
    assert_eq!(chosen, linux.addr);
    assert_eq!(
        mac.requests.lock().unwrap().len(),
        1,
        "stale recommendation was exercised"
    );
    assert_eq!(load_placement(&path).workspaces[&key], linux.addr);
    assert_eq!(
        pick_node_with(&nodes, "apple", Some("go"), Some("macos"), None)
            .await
            .unwrap(),
        mac.addr
    );
    linux.stop().await;
    mac.stop().await;
}

#[tokio::test]
async fn issue_1330_recommendation_respects_preference_and_pressure() {
    let mac = Node::new("macos aarch64", &["go"], true).await;
    let linux = Node::new("linux x86_64", &["go"], false).await;
    let nodes = [mac.addr, linux.addr];
    let key = mac_first(&nodes, mac.addr);
    assert_eq!(
        pick_node_with(&nodes, &key, Some("go"), None, None)
            .await
            .unwrap(),
        linux.addr
    );
    assert_eq!(mac.requests.lock().unwrap().len(), 1);
    linux.state.lock().unwrap().host.storage_free_millis = Some(1);
    assert_eq!(
        pick_node_with(&nodes, &key, Some("go"), None, None)
            .await
            .unwrap(),
        mac.addr
    );
    linux.state.lock().unwrap().host.storage_free_millis = Some(800);
    linux.state.lock().unwrap().detected_engines = vec!["python".into()];
    assert_eq!(
        pick_node_with(&nodes, &key, Some("go"), None, None)
            .await
            .unwrap(),
        mac.addr
    );
    let err = pick_node_with(&[linux.addr], &key, Some("go"), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("serves go"));
    // With placement unavailable, the candidate pool also prefers capacity over OS.
    linux.state.lock().unwrap().detected_engines = vec!["go".into()];
    linux.state.lock().unwrap().host.storage_free_millis = Some(1);
    let fallback_mac = Node::new("macos aarch64", &["go"], false).await;
    assert_eq!(
        pick_node_with(
            &[linux.addr, fallback_mac.addr],
            &key,
            Some("go"),
            None,
            None
        )
        .await
        .unwrap(),
        fallback_mac.addr
    );
    fallback_mac.stop().await;
    linux.stop().await;
    mac.stop().await;
}
