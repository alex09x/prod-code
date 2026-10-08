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
fn serialized_frame_bounding_preserves_non_tool_responses() {
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
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "tools": big_tools }
    });

    let serialized = bound_serialized_response(response, MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.contains("tools"));
    assert!(serialized.contains("tool_0"));
    assert!(!serialized.contains("output truncated to avoid exceeding MCP frame line limits"));
}
