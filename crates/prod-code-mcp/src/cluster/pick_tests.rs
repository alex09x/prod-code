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
use prod_code_protocol::{PlaceResponse, ProdCodeCodec, StatusResponse, WireMessage};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::codec::Framed;

struct Node {
    addr: SocketAddr,
    placements: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Node {
    async fn start(placement: Option<SocketAddr>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let placements = Arc::new(AtomicUsize::new(0));
        let counted = placements.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                let reply = match framed.next().await {
                    Some(Ok(WireMessage::StatusRequest)) => {
                        WireMessage::StatusResponse(StatusResponse {
                            detected_engines: vec!["rust".into()],
                            ..Default::default()
                        })
                    }
                    Some(Ok(WireMessage::PlaceRequest(_))) => {
                        counted.fetch_add(1, Ordering::SeqCst);
                        WireMessage::PlaceResponse(PlaceResponse {
                            node: placement.map(|node| node.to_string()),
                            reason: "test placement".into(),
                            notified_sessions: 0,
                        })
                    }
                    _ => continue,
                };
                let _ = framed.send(reply).await;
            }
        });
        Self {
            addr,
            placements,
            task,
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn issue_1807_pinned_dns_candidates_do_not_follow_unlisted_placement() {
    let outside = Node::start(None).await;
    let first = Node::start(Some(outside.addr)).await;
    let second = Node::start(Some(outside.addr)).await;
    let candidates = [first.addr, second.addr];

    let ordinary = pick_node_with(&candidates, "ordinary", Some("rust"), None, None)
        .await
        .unwrap();
    assert_eq!(
        ordinary, outside.addr,
        "the fixture must offer an unlisted node"
    );
    let previous =
        first.placements.load(Ordering::SeqCst) + second.placements.load(Ordering::SeqCst);
    assert!(previous > 0);

    let pinned = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        pick_node_policy(&candidates, "pinned", Some("rust"), None, None, false),
    )
    .await
    .expect("pinned candidate probing must remain bounded")
    .unwrap();
    assert!(candidates.contains(&pinned));
    assert_eq!(
        first.placements.load(Ordering::SeqCst) + second.placements.load(Ordering::SeqCst),
        previous,
        "a pinned lookup must not request active cluster placement"
    );
}

#[tokio::test]
async fn issue_1807_pinned_address_fallback_only_probes_the_given_host() {
    let live = Node::start(None).await;
    let refused = SocketAddr::new(std::net::Ipv6Addr::LOCALHOST.into(), live.addr.port());
    let nodes = [refused, live.addr];
    let selected = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        pick_node_policy(&nodes, "dual-stack-pin", Some("rust"), None, None, false),
    )
    .await
    .expect("address fallback must remain bounded")
    .unwrap();
    assert_eq!(selected, live.addr);
    assert_eq!(live.placements.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn issue_1807_explicit_loopback_does_not_use_saved_other_node() {
    let outside = Node::start(None).await;
    let directory = tempfile::tempdir().unwrap();
    let placement_file = directory.path().join("placement.json");
    std::fs::write(
        &placement_file,
        serde_json::json!({"workspaces": {"pinned": outside.addr}}).to_string(),
    )
    .unwrap();
    let loopback: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let selected = pick_node_policy(
        &[loopback],
        "pinned",
        None,
        None,
        Some(&placement_file),
        false,
    )
    .await
    .unwrap();
    assert_eq!(selected, loopback);
}
