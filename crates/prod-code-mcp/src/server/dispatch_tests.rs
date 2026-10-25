/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `tools/call` on the paths where the gateway is slow or gone: the call budget, and the search
//! for another node that a connection failure sets off.

use super::dispatch::{
    MAX_TOOL_CALL_TIMEOUT, MCP_TOOL_CALL_TIMEOUT, handle_mcp_request_with,
    resolve_tool_call_timeout,
};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, StatusResponse, WireMessage};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

const REPORT_HINT: &str =
    "\n\n💡 If this is an unexpected error or a bug in prod-code, please report it:";

fn status() -> StatusResponse {
    StatusResponse {
        server_pid: 4242,
        uptime_seconds: 3725,
        active_sessions: 1,
        loaded_workspaces: 1,
        detected_engines: vec!["rust (ra_ap_ide)".to_string()],
        memory_rss_bytes: Some(1024 * 1024),
        total_queries: 5,
        active_queries: 0,
        load_average_millis: None,
        cpu_count: Some(8),
        platform: None,
        running_commands: Vec::new(),
        host: Default::default(),
        version: None,
        git_commit: None,
    }
}

/// A node that answers one status request, or that takes the connection and never answers.
async fn node(answers: bool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        let _ = framed.next().await;
        if answers {
            let _ = framed.send(WireMessage::StatusResponse(status())).await;
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    addr
}

/// The search for another node, where none may be made.
fn never() -> Option<SocketAddr> {
    panic!("a slow gateway is not an unreachable one")
}

fn status_call(id: u64, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": "code_status", "arguments": arguments }
    })
}

#[test]
fn the_call_budget_comes_from_milliseconds_then_seconds_and_is_capped() {
    let secs = |s: u64| Duration::from_secs(s);
    let cases = [
        (json!({}), MCP_TOOL_CALL_TIMEOUT),
        (json!({ "timeout_secs": 10 }), secs(15)),
        (json!({ "timeout_secs": 0 }), MCP_TOOL_CALL_TIMEOUT),
        (json!({ "timeout_secs": "10" }), MCP_TOOL_CALL_TIMEOUT),
        (json!({ "timeout_millis": 250 }), Duration::from_millis(250)),
        (
            json!({ "timeout_millis": 250, "timeout_secs": 10 }),
            Duration::from_millis(250),
        ),
        (json!({ "timeout_millis": 0, "timeout_secs": 10 }), secs(15)),
        (json!({ "timeout_secs": u64::MAX }), MAX_TOOL_CALL_TIMEOUT),
        (json!({ "timeout_millis": u64::MAX }), MAX_TOOL_CALL_TIMEOUT),
        (json!({ "timeout_secs": 24 * 3600 }), MAX_TOOL_CALL_TIMEOUT),
    ];
    for (arguments, expected) in cases {
        assert_eq!(
            resolve_tool_call_timeout(&arguments),
            expected,
            "{arguments}"
        );
    }
}

#[tokio::test]
async fn a_call_that_outlives_its_budget_is_answered_as_a_failed_tool() {
    let mut remote = node(false).await;
    let root = PathBuf::from("/tmp");
    let call = status_call(9, json!({ "timeout_millis": 50 }));
    let response = handle_mcp_request_with(&mut remote, &root, call, |_, _| async { never() })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(response["id"], 9);
    assert_eq!(response["result"]["isError"], true);
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with(&format!(
            "Tool call 'code_status' timed out after 0s{REPORT_HINT}"
        )),
        "{text}"
    );
    assert!(text.contains("code_report_issue"), "{text}");
}

#[tokio::test]
async fn an_unreachable_gateway_is_replaced_by_the_node_discovery_finds_and_the_call_retried() {
    let live = node(true).await;
    let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let mut remote = dead;
    let root = PathBuf::from("/tmp");
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = asked.clone();
    let response = handle_mcp_request_with(
        &mut remote,
        &root,
        status_call(3, json!({})),
        move |old, search_root| {
            counted.fetch_add(1, Ordering::SeqCst);
            assert_eq!((old, search_root), (dead, PathBuf::from("/tmp")));
            async move { Some(live) }
        },
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(remote, live, "the session moves to the node that answered");
    assert_eq!(response["id"], 3);
    assert_eq!(response["result"]["isError"], false);
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with(&format!("⚡ prod-code Gateway Status\n• Address: {live} (")),
        "{text}"
    );
    assert!(
        text.contains("• Server PID: 4242\n• Uptime: 1h 2m 5s\n• Memory RSS: 1.00 MB"),
        "{text}"
    );
    assert!(
        text.ends_with("• Engines: rust (ra_ap_ide)\n• Status: HEALTHY"),
        "{text}"
    );
}

#[tokio::test]
async fn without_another_node_the_connection_error_is_what_the_caller_sees() {
    let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let mut remote = dead;
    let root = PathBuf::from("/tmp");
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = asked.clone();
    let response = handle_mcp_request_with(
        &mut remote,
        &root,
        status_call(4, json!({})),
        move |_, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            async { None }
        },
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(remote, dead);
    assert_eq!(response["result"]["isError"], true);
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with(&format!(
            "Failed to connect to gateway at {dead}{REPORT_HINT}"
        )),
        "{text}"
    );

    // A command is never replayed on another node: no search is made for it.
    let exec = json!({
        "jsonrpc": "2.0", "id": 5, "method": "tools/call",
        "params": { "name": "code_exec", "arguments": { "argv": ["true"] } }
    });
    let counted = asked.clone();
    let response = handle_mcp_request_with(&mut remote, &root, exec, move |_, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        async { None }
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert_eq!(response["result"]["isError"], true);
}
