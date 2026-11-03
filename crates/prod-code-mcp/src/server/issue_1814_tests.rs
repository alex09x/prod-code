/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1814: a small `budget_ms` holds from the moment the stdio loop reads the request. A slow
//! cluster probe does not stall the loop, a failover retry does not get a second budget, and a
//! slow analyzer is not taken for a broken node.

use super::dispatch::handle_mcp_request_with;
use super::rebalance_task::Rebalancer;
use super::reload::Reload;
use super::transport::serve_with_reload;
use super::transport_serve_tests::{next_response, send};
use crate::cluster::{Endpoint, NodePolicy, PinMode};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::BufReader;

const CARGO: &str = "[package]\nname = \"issue_1814\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str = "pub fn target() {}\n";

fn text(response: &serde_json::Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A node that takes every status request and never answers it, as an overloaded gateway
/// does: each probe of the rebalance tick waits out its own reply timeout. Resolves `probed`
/// when the first probe connects.
async fn stalled_node(probed: Arc<tokio::sync::Notify>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            probed.notify_one();
            held.push(socket);
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_1814_fast_calls_are_answered_while_a_rebalance_status_probe_stalls() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/lib.rs", LIB)]);
    let probed = Arc::new(tokio::sync::Notify::new());
    let node = stalled_node(probed.clone()).await;
    let mut reload = Reload::new(None);
    reload.rebalance_every = Duration::from_millis(20);
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (read, write) = tokio::io::split(server);
    let handle = tokio::spawn(serve_with_reload(
        node,
        ws.root(),
        false,
        BufReader::new(read),
        write,
        reload,
    ));
    let mut client = BufReader::new(client);
    tokio::time::timeout(Duration::from_secs(5), probed.notified())
        .await
        .expect("the rebalance tick asks the node for its status");

    let sent = Instant::now();
    send(&mut client, r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).await;
    // A call refused before it reaches any node: its answer comes from a tool task, so it
    // also shows that finished calls are delivered while the probe waits.
    send(
        &mut client,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"code_definition","arguments":{"line":0,"character":1,"budget_ms":600}}}"#,
    )
    .await;
    let mut ids = Vec::new();
    for _ in 0..2 {
        let response =
            tokio::time::timeout(Duration::from_millis(1_500), next_response(&mut client))
                .await
                .expect("the stdio loop answers while the status probe stalls");
        ids.push(response["id"].as_i64().unwrap());
    }
    ids.sort();
    assert_eq!(ids, [1, 2]);
    let elapsed = sent.elapsed();
    assert!(
        elapsed < Duration::from_millis(600),
        "fast calls waited {elapsed:?} behind the rebalance probe"
    );
    drop(client);
    let stopped = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(
        stopped.is_ok(),
        "the loop stops at the end of input without waiting for the probe"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_1814_a_pinned_workspace_starts_no_background_probe() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/lib.rs", LIB)]);
    let probed = Arc::new(tokio::sync::Notify::new());
    let node = stalled_node(probed.clone()).await;
    let mut rebalancer = Rebalancer::default();
    for mode in [PinMode::Strict, PinMode::Listed] {
        let pinned = NodePolicy {
            mode,
            endpoints: vec![Endpoint {
                name: node.to_string(),
                addrs: vec![node],
            }],
            source: "--pin".into(),
        };
        rebalancer.start_under(&pinned, node, &ws.root());
        assert!(!rebalancer.is_running(), "{mode:?}");
    }
    let quiet = tokio::time::timeout(Duration::from_millis(300), probed.notified()).await;
    assert!(
        quiet.is_err(),
        "a pinned workspace asked its node for status"
    );
    // Unpinned, the same start probes the node, beside the caller.
    rebalancer.start_under(&NodePolicy::default(), node, &ws.root());
    assert!(rebalancer.is_running());
    tokio::time::timeout(Duration::from_secs(5), probed.notified())
        .await
        .expect("an unpinned workspace is checked");
}

#[tokio::test]
async fn issue_1814_a_rebalance_verdict_applies_only_to_the_node_it_asked_about() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/lib.rs", LIB)]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let better = listener.local_addr().unwrap();
    let asked_about: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let failed_over: SocketAddr = "127.0.0.1:2".parse().unwrap();
    let verdict = || (asked_about, Some((better, "roomier".to_string())));
    // A call failed over while the cluster was asked: the verdict no longer applies.
    let mut remote = failed_over;
    assert!(!Rebalancer::apply(&mut remote, verdict(), &ws.root()));
    assert_eq!(remote, failed_over);
    // No verdict: the session stays.
    let mut remote = asked_about;
    assert!(!Rebalancer::apply(
        &mut remote,
        (asked_about, None),
        &ws.root()
    ));
    assert_eq!(remote, asked_about);
    // Still on the node it asked about: the session moves and the new node is pre-warmed.
    assert!(Rebalancer::apply(&mut remote, verdict(), &ws.root()));
    assert_eq!(remote, better);
    let warmed = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await;
    assert!(
        warmed.is_ok(),
        "the new node is connected to for the pre-warm sync"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_1814_a_failover_retry_spends_what_is_left_of_the_first_budget() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/lib.rs", LIB)]);
    // The node discovery finds never answers a navigation request.
    let stalled = ScriptedGateway::start(|method, _| {
        if method.starts_with("textDocument/") || method.starts_with("workspace/") {
            json!({ "prod-code/no-response": true })
        } else {
            serde_json::Value::Null
        }
    })
    .await;
    let live = stalled.addr();
    let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let mut remote = dead;
    let call = json!({
        "jsonrpc": "2.0", "id": 11, "method": "tools/call",
        "params": { "name": "code_definition", "arguments": {
            "path": "src/lib.rs", "line": 1, "character": 8, "body": false, "budget_ms": 600
        }}
    });
    let started = Instant::now();
    let response = handle_mcp_request_with(&mut remote, &ws.root(), call, move |_, _| async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        Some(live)
    })
    .await
    .unwrap()
    .unwrap();
    let elapsed = started.elapsed();
    let text = text(&response);
    assert_eq!(
        remote, live,
        "the retry ran on the node discovery found: {text}"
    );
    assert_eq!(response["result"]["isError"], true, "{text}");
    assert!(text.contains("600 ms"), "{text}");
    assert!(
        elapsed < Duration::from_millis(900),
        "the retry was given a fresh budget: {elapsed:?} {text}"
    );
}

#[test]
fn issue_1814_an_analyzer_that_is_slow_or_busy_is_not_a_broken_node() {
    let unpinned = crate::cluster::NodePolicy::default();
    for message in [
        "timeout waiting for textDocument/hover",
        "timeout waiting for another query in this workspace after 40 ms; the gateway may be \
         loading or short of capacity. Run `prod-code cluster` to inspect it",
        "timeout running textDocument/references (including sync and engine load) after 30000 \
         ms; the gateway may be loading or short of capacity",
    ] {
        let error = anyhow::anyhow!("{message}").context("tool call failed");
        for tool in ["code_hover", "code_callers", "code_definition"] {
            assert!(
                !super::failover::is_retryable_connection_error(tool, &error),
                "{tool}: {message}"
            );
            assert!(
                !super::failover_pin::retry_permitted(&unpinned, tool, &error),
                "{tool}: {message}"
            );
        }
    }
    // A gateway that never took the connection is still one to leave.
    for message in [
        "Failed to connect to remote gateway at 127.0.0.1:9400: timed out",
        "timed out connecting to gateway 127.0.0.1:9400",
        "request timeout",
    ] {
        let error = anyhow::anyhow!("{message}");
        assert!(
            super::failover::is_retryable_connection_error("code_hover", &error),
            "{message}"
        );
    }
}
