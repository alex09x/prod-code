/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::hot_reload;
use crate::server::dispatch::handle_mcp_request;
use crate::server::failover::is_retryable_connection_error;
use crate::server::transport::serve_mcp_requests;

#[tokio::test]
async fn test_mcp_initialize() {
    let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let root = PathBuf::from("/tmp");

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {}
        }
    });

    let resp_opt = handle_mcp_request(&mut dummy_addr, &root, req)
        .await
        .unwrap();
    assert!(resp_opt.is_some());
    let resp = resp_opt.unwrap();
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["serverInfo"]["name"], "prod-code-mcp");
    assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    // Every agent reads these, skill or not: they carry the bug-report rule too (#302).
    let instructions = resp["result"]["instructions"].as_str().unwrap_or_default();
    for needed in [
        "code_report_issue",
        "private_ref",
        "never put private details",
        "labels",
        "code_change_signature",
        "code_replace_constructor_with_builder",
        "code_shadow_run",
        "code_validate_edits",
        "code_prune_orphans",
    ] {
        assert!(
            instructions.contains(needed),
            "{needed} missing: {instructions}"
        );
    }
}

#[tokio::test]
async fn test_mcp_tools_list() {
    let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let root = PathBuf::from("/tmp");

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });

    let resp_opt = handle_mcp_request(&mut dummy_addr, &root, req)
        .await
        .unwrap();
    assert!(resp_opt.is_some());
    let resp = resp_opt.unwrap();
    assert_eq!(resp["id"], 2);
    let tools = resp["result"]["tools"].as_array().unwrap();
    let tool_names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    assert!(tool_names.contains(&"code_definition"));
    assert!(tool_names.contains(&"code_references"));
    assert!(tool_names.contains(&"code_outline"));
    assert!(tool_names.contains(&"code_hover"));
    assert!(tool_names.contains(&"code_status"));
    assert!(tool_names.contains(&"code_sync"));
}

#[tokio::test]
async fn test_mcp_ping() {
    let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let root = PathBuf::from("/tmp");

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "ping"
    });

    let resp = handle_mcp_request(&mut dummy_addr, &root, req)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp["id"], 3);
    assert_eq!(resp["result"], serde_json::json!({}));
}

#[tokio::test]
async fn test_mcp_unknown_method() {
    let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let root = PathBuf::from("/tmp");

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "foo/bar"
    });

    let resp = handle_mcp_request(&mut dummy_addr, &root, req)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp["id"], 4);
    assert_eq!(resp["error"]["code"], -32601);
}

#[tokio::test]
async fn test_mcp_unknown_notification_gets_no_response() {
    let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let root = PathBuf::from("/tmp");

    let req = serde_json::json!({ "jsonrpc": "2.0", "method": "foo/bar" });

    let resp = handle_mcp_request(&mut dummy_addr, &root, req)
        .await
        .unwrap();
    assert!(resp.is_none(), "a notification gets no reply: {resp:?}");
}

#[tokio::test]
async fn test_mcp_initialized_notification_gets_no_response() {
    let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let root = PathBuf::from("/tmp");

    for method in ["notifications/initialized", "initialized"] {
        let req = serde_json::json!({ "jsonrpc": "2.0", "method": method });
        let resp = handle_mcp_request(&mut dummy_addr, &root, req)
            .await
            .unwrap();
        assert!(resp.is_none(), "{method}: {resp:?}");
    }
}

#[tokio::test]
async fn test_mcp_malformed_request_is_a_parse_error() {
    let mut dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let root = PathBuf::from("/tmp");

    // Missing the required `method` field: the request itself does not deserialize.
    let resp = handle_mcp_request(&mut dummy_addr, &root, serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp["error"]["code"], -32700);
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Parse error:"),
        "{resp:?}"
    );
}

#[tokio::test]
async fn test_mcp_tools_call_reports_a_failed_tool_as_a_normal_response() {
    // A closed port: the tool's own connection attempt fails, and that failure is
    // reported as a normal (non-transport) JSON-RPC response, not a handler error. Port 1,
    // not a port bound and dropped: with the whole suite running in parallel, a freed
    // ephemeral port is taken by another test's listener often enough that the connection
    // succeeds and is reset instead of refused.
    let mut dummy_addr: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    let root = PathBuf::from("/tmp");

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": { "name": "code_exec", "arguments": { "argv": ["true"] } }
    });

    let resp = handle_mcp_request(&mut dummy_addr, &root, req)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp["id"], 5);
    assert_eq!(resp["result"]["isError"], true);
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.to_lowercase().contains("failed to connect"), "{text}");
}

/// Wires `serve_mcp_requests` to an in-memory duplex stream: the returned handle is the
/// client's end (write requests into it, read responses back out of it), and the join
/// handle resolves once the client end (or its clone) is dropped, which the server reads
/// as EOF.
fn spawn_server(resumed: bool) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<Result<()>>) {
    let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let root = PathBuf::from("/tmp");
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let handle = tokio::spawn(serve_mcp_requests(
        dummy_addr,
        root,
        None, // no real executable to hot-reload from in a test
        resumed,
        BufReader::new(server_read),
        server_write,
    ));
    (client, handle)
}

#[tokio::test]
async fn serve_mcp_requests_answers_a_request_then_stops_at_eof() {
    let (mut client, handle) = spawn_server(false);
    client
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
        .await
        .unwrap();

    let mut buf = [0u8; 4096];
    let n = client.read(&mut buf).await.unwrap();
    let line = String::from_utf8_lossy(&buf[..n]);
    let resp: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"], serde_json::json!({}));

    drop(client); // EOF: the server's read half sees a closed connection
    let result = handle.await.unwrap();
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn serve_mcp_requests_skips_blank_lines_and_invalid_json() {
    let (mut client, handle) = spawn_server(false);
    client
        .write_all(b"\n   \nnot json at all\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n")
        .await
        .unwrap();

    // Read all output: blank lines are skipped, invalid JSON produces a parse error
    // response (-32700 with no id), and the valid ping produces a normal response.
    let mut all = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = client.read(&mut buf).await.unwrap();
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&all);
        // Stop once we've seen the ping response (id:2).
        if text.contains("\"id\":2") || text.contains("\"id\": 2") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&all);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    // Two response lines: parse error + ping.
    assert!(lines.len() >= 2, "expected ≥2 responses: {text:?}");
    let parse_err: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(parse_err["error"]["code"], -32700);
    let ping: serde_json::Value = serde_json::from_str(lines[lines.len() - 1]).unwrap();
    assert_eq!(ping["id"], 2);
    assert_eq!(ping["result"], serde_json::json!({}));

    drop(client);
    assert!(handle.await.unwrap().is_ok());
}

#[tokio::test]
async fn serve_mcp_requests_announces_a_resumed_session_before_any_request() {
    let (mut client, handle) = spawn_server(true);

    let mut buf = [0u8; 4096];
    let n = client.read(&mut buf).await.unwrap();
    let line = String::from_utf8_lossy(&buf[..n]);
    assert_eq!(line, hot_reload::tools_list_changed());

    drop(client);
    assert!(handle.await.unwrap().is_ok());
}

#[test]
fn test_is_retryable_connection_error_rules() {
    let connect_err =
        anyhow::anyhow!("Failed to connect to remote gateway: Connection refused (os error 111)");
    let exec_unknown_err = anyhow::anyhow!(
        "lost the connection to the gateway during exec (connection reset); the command's result is unknown"
    );
    let generic_err = anyhow::anyhow!("syntax error in file.rs");
    let capacity_err = anyhow::anyhow!(
        "the gateway refused the session: capacity: this node has no memory for a new rust engine (memory 80% used)"
    );
    let transport_closed_err =
        anyhow::anyhow!("tool call failed for prod-code/code_definition: Transport closed");
    let broken_pipe_err = anyhow::anyhow!("broken pipe");

    // 1. code_exec must NEVER be retried even on connection refused or capacity
    assert!(!is_retryable_connection_error("code_exec", &connect_err));
    assert!(!is_retryable_connection_error("code_exec", &capacity_err));

    // 2. Unknown outcome during/after exec must NEVER be retried
    assert!(!is_retryable_connection_error(
        "code_check",
        &exec_unknown_err
    ));
    assert!(!is_retryable_connection_error(
        "code_definition",
        &exec_unknown_err
    ));

    // 3. Pre-dispatch connection errors for read-only tools ARE retryable
    assert!(is_retryable_connection_error(
        "code_definition",
        &connect_err
    ));
    assert!(is_retryable_connection_error(
        "code_references",
        &connect_err
    ));
    assert!(is_retryable_connection_error("code_symbols", &capacity_err));
    assert!(is_retryable_connection_error(
        "code_definition",
        &transport_closed_err
    ));
    assert!(is_retryable_connection_error(
        "code_definition",
        &broken_pipe_err
    ));

    // 4. Non-connection errors are not retryable
    assert!(!is_retryable_connection_error(
        "code_definition",
        &generic_err
    ));
}

#[tokio::test]
async fn serve_mcp_requests_recovers_from_panic_and_returns_jsonrpc_error() {
    let (client, handle) = spawn_server(false);
    let mut reader = tokio::io::BufReader::new(client);

    // Ping works normally
    reader
        .get_mut()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
        .await
        .unwrap();

    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let resp: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"], serde_json::json!({}));

    // Send a tool call with unknown tool, server handles it cleanly without dying
    line.clear();
    reader
        .get_mut()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"non_existent_tool_12345\"}}\n")
        .await
        .unwrap();
    reader.read_line(&mut line).await.unwrap();
    let resp: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(resp["id"], 2);

    // Server is still alive: ping succeeds
    line.clear();
    reader
        .get_mut()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n")
        .await
        .unwrap();
    reader.read_line(&mut line).await.unwrap();
    let resp: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(resp["id"], 3);

    drop(reader);
    assert!(handle.await.unwrap().is_ok());
}
