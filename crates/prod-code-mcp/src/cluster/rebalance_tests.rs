/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ClusterResponse, HostResources, PeerInfo, ProdCodeCodec, StatusResponse, WireMessage,
};
use tokio_util::codec::Framed;

fn status(load: u32, pressure: bool) -> StatusResponse {
    StatusResponse {
        detected_engines: vec!["rust".into()],
        platform: Some("linux x86_64".into()),
        cpu_count: Some(1),
        load_average_millis: Some(load),
        host: HostResources {
            memory_available_bytes: Some(80),
            memory_total_bytes: Some(100),
            storage_free_millis: Some(if pressure { 20 } else { 800 }),
        },
        ..Default::default()
    }
}

struct Node {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Node {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Node {
    async fn new(status: StatusResponse, peers: Option<Vec<PeerInfo>>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                match framed.next().await {
                    Some(Ok(WireMessage::StatusRequest)) => {
                        let _ = framed
                            .send(WireMessage::StatusResponse(status.clone()))
                            .await;
                    }
                    Some(Ok(WireMessage::ClusterRequest)) => {
                        if let Some(peers) = &peers {
                            let _ = framed
                                .send(WireMessage::ClusterResponse(ClusterResponse {
                                    this_node: addr.to_string(),
                                    nodes: peers.clone(),
                                }))
                                .await;
                        }
                    }
                    _ => {}
                }
            }
        });
        Self { addr, task }
    }
}

fn peer(addr: &str, status: StatusResponse, alive: bool) -> PeerInfo {
    PeerInfo {
        addr: addr.into(),
        status,
        alive,
        last_seen_secs: 0,
        workspaces: vec![],
    }
}

#[tokio::test]
async fn runtime_paths_2_rebalance_view_filters_and_formats_migration() {
    let chosen: SocketAddr = "127.0.0.1:9001".parse().unwrap();
    let mut mac = status(0, false);
    mac.platform = Some("macos aarch64".into());
    let mut wrong = status(0, false);
    wrong.detected_engines = vec!["python".into()];
    let peers = vec![
        peer("not an address", status(0, false), true),
        peer("127.0.0.1:9002", status(0, false), false),
        peer("127.0.0.1:9003", mac, true),
        peer("127.0.0.1:9004", wrong, true),
        peer("127.0.0.1:9005", status(0, true), true),
        peer(&chosen.to_string(), status(100, false), true),
    ];
    for pressured in [false, true] {
        let current = Node::new(status(1000, pressured), Some(peers.clone())).await;
        let result = evaluate_cluster_rebalance_with(
            &[current.addr, chosen],
            current.addr,
            "sample",
            Some("rust"),
            None,
        )
        .await;
        let reason = if pressured {
            format!(
                "node {} is under resource pressure (disk 2% free), migrating to roomier {chosen} (score 0.10, memory 20% used, disk 80% free)",
                current.addr
            )
        } else {
            format!(
                "rebalancing from congested node {} (score 1.00) to quieter {chosen} (score 0.10, memory 20% used, disk 80% free)",
                current.addr
            )
        };
        assert_eq!(result, Some((chosen, reason)));
    }
}

#[tokio::test]
async fn runtime_paths_2_rebalance_hysteresis_empty_and_pressure_pools() {
    let candidate = "127.0.0.1:9001";
    for (current_load, current_pressure, peer_load, peer_pressure, include_peer) in [
        (100, false, 0, false, true),    // Healthy: retain caches.
        (1000, false, 600, false, true), // Less than a twofold improvement.
        (1000, true, 0, true, true),     // All nodes are pressured.
        (1000, false, 0, false, false),  // Empty cluster view.
    ] {
        let peers = if include_peer {
            vec![peer(candidate, status(peer_load, peer_pressure), true)]
        } else {
            vec![]
        };
        let current = Node::new(status(current_load, current_pressure), Some(peers)).await;
        assert_eq!(
            evaluate_cluster_rebalance_with(
                &[current.addr, candidate.parse().unwrap()],
                current.addr,
                "sample",
                Some("rust"),
                None
            )
            .await,
            None
        );
    }
}

#[tokio::test]
async fn runtime_paths_2_rebalance_direct_probe_and_unreachable_current() {
    let linux = Node::new(status(100, false), None).await;
    let mut mac_status = status(0, false);
    mac_status.platform = Some("macos aarch64".into());
    let mac = Node::new(mac_status, None).await;
    let current = Node::new(status(1000, false), None).await;
    let result = evaluate_cluster_rebalance_with(
        &[current.addr, mac.addr, linux.addr],
        current.addr,
        "sample",
        Some("rust"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        result,
        (
            linux.addr,
            format!(
                "rebalancing from congested node {} (score 1.00) to quieter {} (score 0.10, memory 20% used, disk 80% free)",
                current.addr, linux.addr
            )
        )
    );
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead = closed.local_addr().unwrap();
    drop(closed);
    for (nodes, expected) in [
        (vec![dead, mac.addr, linux.addr], Some(linux.addr)),
        (vec![dead, mac.addr], Some(mac.addr)),
        (vec![dead], None),
    ] {
        let actual =
            evaluate_cluster_rebalance_with(&nodes, dead, "sample", Some("rust"), None).await;
        assert_eq!(
            actual,
            expected.map(|addr| (
                addr,
                format!("current node {dead} is unreachable, migrating to live peer {addr}")
            ))
        );
    }
}

#[tokio::test]
async fn runtime_paths_2_pick_rebalances_remembered_congested_node_and_saves_choice() {
    use crate::cluster::pick::pick_node_with;
    use crate::cluster::placement::{Placement, load_placement, save_placement};
    let quiet = Node::new(status(100, false), None).await;
    let current = Node::new(
        status(1000, false),
        Some(vec![peer(
            &quiet.addr.to_string(),
            status(100, false),
            true,
        )]),
    )
    .await;
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("placement.json");
    let mut placement = Placement::default();
    placement.workspaces.insert("sample".into(), current.addr);
    save_placement(&file, &placement);
    assert_eq!(
        pick_node_with(
            &[current.addr, quiet.addr],
            "sample",
            Some("rust"),
            None,
            Some(&file)
        )
        .await
        .unwrap(),
        quiet.addr
    );
    assert_eq!(
        load_placement(&file).workspaces.get("sample"),
        Some(&quiet.addr)
    );
    assert_eq!(
        pick_node_with(&[quiet.addr], "single", Some("rust"), None, None)
            .await
            .unwrap(),
        quiet.addr
    );
    assert_eq!(
        pick_node_with(&[], "empty", None, None, None)
            .await
            .unwrap_err()
            .to_string(),
        "no gateway addresses given"
    );
}
