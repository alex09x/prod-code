/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[test]
fn serialized_frame_bounding_with_escape_heavy_output() {
    use crate::server::transport::{MAX_JSONRPC_FRAME_BYTES, bound_serialized_response};

    // 40,000 newlines escape to 80,000 bytes in JSON ("\n" -> "\\n")
    let raw = "\n".repeat(40_000);
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "content": [{
                "type": "text",
                "text": raw
            }],
            "isError": false
        }
    });

    let serialized = bound_serialized_response(response, MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.len() <= MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.contains("output truncated"));
}

#[test]
fn serialized_frame_bounding_returns_error_for_oversized_non_tool_responses() {
    use crate::server::transport::{MAX_JSONRPC_FRAME_BYTES, bound_serialized_response};

    // An oversized tools/list response cannot be represented as a bounded tool content result.
    let big_tools: Vec<serde_json::Value> = (0..200)
        .map(|i| {
            serde_json::json!({
                "name": format!("tool_{i}"),
                "description": "x".repeat(400),
                "inputSchema": { "type": "object" }
            })
        })
        .collect();

    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "tools": big_tools
        }
    });

    let serialized = bound_serialized_response(resp.clone(), MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.len() <= MAX_JSONRPC_FRAME_BYTES);
    let bounded: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(bounded["jsonrpc"], "2.0");
    assert_eq!(bounded["id"], 1);
    assert_eq!(bounded["error"]["code"], -32000);
    assert_eq!(
        bounded["error"]["message"],
        "Response exceeds maximum JSON-RPC frame size"
    );
}

#[test]
fn serialized_frame_bounding_falls_back_to_null_id_when_request_id_is_oversized() {
    use crate::server::transport::{MAX_JSONRPC_FRAME_BYTES, bound_serialized_response};

    let big_tools: Vec<serde_json::Value> = (0..200)
        .map(|i| {
            serde_json::json!({
                "name": format!("tool_{i}"),
                "description": "x".repeat(400),
                "inputSchema": { "type": "object" }
            })
        })
        .collect();

    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "a".repeat(MAX_JSONRPC_FRAME_BYTES * 2),
        "result": {
            "tools": big_tools
        }
    });

    let serialized = bound_serialized_response(resp, MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.len() <= MAX_JSONRPC_FRAME_BYTES);
    let bounded: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(bounded["jsonrpc"], "2.0");
    assert_eq!(bounded["id"], serde_json::Value::Null);
    assert_eq!(bounded["error"]["code"], -32000);
}

#[tokio::test]
async fn cluster_rebalance_tick_ignores_filesystem_root() {
    let mut remote: std::net::SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let initial_remote = remote;
    crate::server::rebalance::handle_cluster_rebalance_tick(&mut remote, std::path::Path::new("/"))
        .await;
    assert_eq!(
        remote, initial_remote,
        "filesystem root must never trigger rebalance"
    );
}

#[tokio::test]
async fn cluster_rebalance_tick_ignores_unbounded_non_git_manifestless_dir() {
    let temp = tempfile::tempdir().unwrap();
    let mut remote: std::net::SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let initial_remote = remote;
    crate::server::rebalance::handle_cluster_rebalance_tick(&mut remote, temp.path()).await;
    assert_eq!(
        remote, initial_remote,
        "manifestless non-git root must not trigger rebalance"
    );
}
