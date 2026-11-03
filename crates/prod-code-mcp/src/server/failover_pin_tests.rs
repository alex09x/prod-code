/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Failover and MCP retries of a pinned workspace (#1793).

use super::*;
use crate::cluster::{Endpoint, PinMode};
use crate::server::dispatch::handle_mcp_request_policy;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, StatusResponse, WireMessage};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::codec::Framed;

fn strict(addrs: &[SocketAddr]) -> NodePolicy {
    NodePolicy {
        mode: PinMode::Strict,
        endpoints: vec![Endpoint {
            name: "dual.example:9400".into(),
            addrs: addrs.to_vec(),
        }],
        source: "--pin".into(),
    }
}

/// A node that answers status requests and counts its connections.
async fn live_node() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counted = connections.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(WireMessage::StatusRequest)) = framed.next().await {
                    let status = StatusResponse {
                        server_pid: 7,
                        detected_engines: vec!["rust".into()],
                        ..Default::default()
                    };
                    if framed
                        .send(WireMessage::StatusResponse(status))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    (addr, connections)
}

fn dead() -> SocketAddr {
    "127.0.0.1:1".parse().unwrap()
}

fn status_call(id: u64) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": "code_status", "arguments": {} }
    })
}

#[test]
fn issue_1793_only_calls_that_never_started_are_retried_under_a_pin() {
    let pinned = strict(&[dead()]);
    let unpinned = NodePolicy::default();
    let refused = anyhow::anyhow!("failed to connect to remote gateway at 127.0.0.1:1")
        .context("code_symbols");
    let reset = anyhow::anyhow!("connection reset by peer");
    let closed = anyhow::anyhow!("gateway closed the connection during the search");
    assert!(retry_permitted(&pinned, "code_symbols", &refused));
    assert!(!retry_permitted(&pinned, "code_symbols", &reset));
    assert!(!retry_permitted(&pinned, "code_search", &closed));
    // Unpinned keeps its old rules.
    assert!(retry_permitted(&unpinned, "code_search", &closed));
    // A command is never replayed, pinned or not.
    assert!(!retry_permitted(&pinned, "code_exec", &refused));
    assert!(!retry_permitted(&unpinned, "code_exec", &refused));
    let unknown = anyhow::anyhow!(
        "failed to connect to remote gateway; lost the connection during exec; the command's result is unknown"
    );
    assert!(!is_unstarted_connection_error(&unknown));
}

#[tokio::test]
async fn issue_1793_rediscovery_stays_within_the_pinned_endpoint() {
    let (alternative, _) = live_node().await;
    let (unlisted, unlisted_hits) = live_node().await;
    let root = tempfile::tempdir().unwrap();
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = asked.clone();
    let picked = rediscover_pinned(
        dead(),
        root.path(),
        &strict(&[dead(), alternative]),
        move |addr| {
            log.lock().unwrap().push(addr);
            crate::cluster::node_status(addr)
        },
    )
    .await;
    assert_eq!(picked, Some(alternative));
    assert_eq!(*asked.lock().unwrap(), [alternative]);
    assert_eq!(unlisted_hits.load(Ordering::SeqCst), 0);
    let _ = unlisted;

    // With no other address of the endpoint alive there is nowhere to go.
    let none = rediscover_pinned(
        dead(),
        root.path(),
        &strict(&[dead()]),
        crate::cluster::node_status,
    )
    .await;
    assert_eq!(none, None);
}

#[tokio::test]
async fn issue_1793_mcp_retry_moves_only_to_an_allowed_address_and_says_so() {
    let (alternative, _) = live_node().await;
    let (unlisted, unlisted_hits) = live_node().await;
    let root = PathBuf::from("/tmp");
    let policy = strict(&[dead(), alternative]);

    // Rediscovery naming a node outside the pin is ignored: the call fails with its reason.
    let mut remote = dead();
    let response =
        handle_mcp_request_policy(&mut remote, &root, status_call(1), &policy, |_, _| async {
            Some(unlisted)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(remote, dead());
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        unlisted_hits.load(Ordering::SeqCst),
        0,
        "no work reached it"
    );

    // The other address of the pinned endpoint takes the call, and the result names it.
    let response =
        handle_mcp_request_policy(&mut remote, &root, status_call(2), &policy, |_, _| async {
            Some(alternative)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(remote, alternative);
    assert_eq!(response["result"]["isError"], false);
    let content = response["result"]["content"].as_array().unwrap();
    let note = content.last().unwrap()["text"].as_str().unwrap();
    assert!(
        note.contains(&format!("relocated: the gateway at {}", dead())),
        "{note}"
    );
    assert!(note.contains(&format!("it ran on {alternative}")), "{note}");
    assert_eq!(unlisted_hits.load(Ordering::SeqCst), 0);
}
