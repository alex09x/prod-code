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

    // 300,000 newlines escape to 600,000 bytes in JSON ("\n" -> "\\n")
    let raw = "\n".repeat(300_000);
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
    let big_tools: Vec<serde_json::Value> = (0..1500)
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

    let big_tools: Vec<serde_json::Value> = (0..1500)
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

#[test]
fn real_tools_list_fits_comfortably_within_max_jsonrpc_frame_bytes() {
    use crate::server::transport::{MAX_JSONRPC_FRAME_BYTES, bound_serialized_response};
    use crate::tools::list_tools;

    let tools = list_tools();
    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "tools": tools
        }
    });

    let serialized = bound_serialized_response(resp, MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.len() <= MAX_JSONRPC_FRAME_BYTES);
    let val: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(val["jsonrpc"], "2.0");
    assert_eq!(val["id"], 1);
    assert!(val.get("result").is_some());
    assert!(val.get("error").is_none());
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

#[tokio::test]
async fn cluster_rebalance_tick_skips_cgo_check_on_rust_workspace() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"dummy\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let target_dir = temp.path().join("target/debug");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(
        target_dir.join("proc.go"),
        "package proc\n// #include <libproc.h>\nimport \"C\"\n",
    )
    .unwrap();

    // Directly verify that macos_only_cgo ignores the target/ directory
    assert_eq!(crate::sync::macos_only_cgo(temp.path()), None);

    // Verify detected engine is rust, so rebalance tick passes os = None
    let (_, engine) = crate::sync::engine_project(temp.path(), temp.path());
    assert_eq!(engine, Some("rust"));
}
