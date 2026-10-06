/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, WireMessage, supported_protocol_versions};
use std::sync::Arc;
use tokio_util::codec::Framed;

use crate::sync::workspace_identity;

use super::super::pool::{pool_key, pooled_query, pooled_query_with_budget, session_slot};
use super::super::types::LspSession;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opening_a_silent_gateway_has_a_deadline() {
    let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hold = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let _socket = socket;
        std::future::pending::<()>().await;
    });
    let result = LspSession::open_with_budget(
        addr,
        &ws.root(),
        None,
        None,
        std::time::Duration::from_millis(100),
    )
    .await;
    let error = result.err().expect("silent peer must time out").to_string();
    assert!(
        error.contains("opening the session") && error.contains("prod-code cluster"),
        "{error}"
    );
    hold.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_incompatible_gateway_selection_is_refused_before_initialize() {
    let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        loop {
            match framed.next().await.unwrap().unwrap() {
                WireMessage::SyncProbeRequest(req) => {
                    framed
                        .send(WireMessage::SyncProbeResponse(
                            prod_code_protocol::SyncProbeResponse {
                                server_workspace_root: req.client_workspace_root,
                                seeded: false,
                                files_deleted: 0,
                                missing: Vec::new(),
                            },
                        ))
                        .await
                        .unwrap();
                }
                WireMessage::HandshakeRequest(req) => {
                    assert_eq!(req.supported_versions, Some(supported_protocol_versions()));
                    framed
                        .send(WireMessage::HandshakeResponse(
                            prod_code_protocol::HandshakeResponse {
                                protocol_version: 2,
                                server_pid: 1,
                                session_id: 1,
                                server_workspace_root: req.client_workspace_root,
                                detected_engine: "rust".to_string(),
                                stale_paths: Vec::new(),
                                engine_age_ms: None,
                                index_gated: false,
                                capabilities: None,
                            },
                        ))
                        .await
                        .unwrap();
                    break;
                }
                other => panic!("unexpected setup message: {other:?}"),
            }
        }
        assert!(
            framed.next().await.is_none(),
            "an incompatible selection must close before initialize"
        );
    });

    let result = LspSession::open_with_budget(
        addr,
        &ws.root(),
        None,
        None,
        std::time::Duration::from_secs(3),
    )
    .await;
    let error = result
        .err()
        .expect("incompatible selection must fail")
        .to_string();
    assert!(
        error.contains("incompatible MCP handshake response"),
        "{error}"
    );
    peer.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queued_query_times_out_without_discarding_the_owner() {
    let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let gateway = prod_code_testkit::ScriptedGateway::start(|_, _| serde_json::json!([])).await;
    let root = ws.root();
    let file = ws.path("src/lib.rs");
    pooled_query(
        gateway.addr(),
        &root,
        &file,
        "textDocument/documentSymbol",
        serde_json::json!({}),
    )
    .await
    .unwrap();
    let (_, key) = pool_key(gateway.addr(), &root, &file);
    let slot = session_slot(&key).await;
    let held = slot.lock().await;
    assert!(held.is_some());
    let error = pooled_query_with_budget(
        gateway.addr(),
        &root,
        &file,
        "textDocument/documentSymbol",
        serde_json::json!({}),
        std::time::Duration::from_millis(40),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("waiting for another query"), "{error}");
    assert!(
        held.is_some(),
        "the queued caller must not invalidate its owner"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timed_out_query_discards_its_connection_without_replaying_it() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let slow = Arc::new(AtomicBool::new(false));
    let handshakes = Arc::new(AtomicUsize::new(0));
    let (delay, counted) = (Arc::clone(&slow), Arc::clone(&handshakes));
    let gateway = prod_code_testkit::ScriptedGateway::start(move |method, _| {
        if method == "prod-code/handshake" {
            counted.fetch_add(1, Ordering::SeqCst);
        }
        if method == "textDocument/documentSymbol" && delay.swap(false, Ordering::SeqCst) {
            tokio::task::block_in_place(|| {
                std::thread::sleep(std::time::Duration::from_millis(300));
            });
        }
        serde_json::json!([])
    })
    .await;
    let root = ws.root();
    let file = ws.path("src/lib.rs");
    pooled_query(
        gateway.addr(),
        &root,
        &file,
        "textDocument/documentSymbol",
        serde_json::json!({}),
    )
    .await
    .unwrap();
    slow.store(true, Ordering::SeqCst);
    let before = gateway.calls();
    let error = pooled_query_with_budget(
        gateway.addr(),
        &root,
        &file,
        "textDocument/documentSymbol",
        serde_json::json!({}),
        std::time::Duration::from_millis(100),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("timeout running textDocument/documentSymbol"),
        "{error}"
    );
    assert_eq!(
        gateway.calls(),
        before + 1,
        "a timeout must not replay the request"
    );
    let (_, key) = pool_key(gateway.addr(), &root, &file);
    assert!(session_slot(&key).await.lock().await.is_none());
    pooled_query(
        gateway.addr(),
        &root,
        &file,
        "textDocument/documentSymbol",
        serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(
        handshakes.load(Ordering::SeqCst),
        2,
        "the next query reconnects"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn established_session_migrates_when_placement_is_rebalanced() {
    let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let gateway1 = prod_code_testkit::ScriptedGateway::start(|_, _| serde_json::json!([])).await;
    let gateway2 = prod_code_testkit::ScriptedGateway::start(|_, _| serde_json::json!([])).await;
    let root = ws.root();
    let file = ws.path("src/lib.rs");
    let ws_identity = workspace_identity(&root);

    // Query 1: runs on gateway1
    pooled_query(
        gateway1.addr(),
        &root,
        &file,
        "textDocument/documentSymbol",
        serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(gateway1.calls(), 1);
    assert_eq!(gateway2.calls(), 0);

    // Rebalance workspace placement to gateway2
    crate::cluster::remember_placement(&ws_identity.name, gateway2.addr());

    // Query 2: established session on gateway1 must migrate to gateway2
    pooled_query(
        gateway1.addr(),
        &root,
        &file,
        "textDocument/documentSymbol",
        serde_json::json!({}),
    )
    .await
    .unwrap();
    assert_eq!(gateway1.calls(), 1);
    assert_eq!(gateway2.calls(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn established_session_retries_on_mid_session_redirect() {
    let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let gateway2 = prod_code_testkit::ScriptedGateway::start(|_, _| serde_json::json!([])).await;
    let g2_addr = gateway2.addr();

    let gateway1 = prod_code_testkit::ScriptedGateway::start(move |method, _| {
        if method == "textDocument/documentSymbol" {
            serde_json::json!({
                "redirect": g2_addr.to_string(),
                "reason": "congested gateway",
            })
        } else {
            serde_json::json!([])
        }
    })
    .await;

    let root = ws.root();
    let file = ws.path("src/lib.rs");

    let result = pooled_query(
        gateway1.addr(),
        &root,
        &file,
        "textDocument/documentSymbol",
        serde_json::json!({}),
    )
    .await;

    assert!(
        result.is_ok(),
        "query should succeed after following redirect"
    );
    assert_eq!(gateway1.calls(), 1);
    assert_eq!(gateway2.calls(), 1);
}
