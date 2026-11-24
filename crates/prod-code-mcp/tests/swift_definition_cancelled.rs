/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #2083: a warming Swift analyzer cancels `textDocument/definition`; a declaration the file's
//! own outline shows is still answered, labelled syntax-only, instead of the cancellation.

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_mcp::tools::execute_tool;
use prod_code_protocol::{
    HandshakeResponse, PROTOCOL_VERSION, ProdCodeCodec, SyncProbeResponse, SyncResponse,
    WireMessage,
};
use prod_code_testkit::Workspace;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

const SOURCE: &str =
    "struct TerminalController {\n    func newWindow() { print(\"ok\") }\n}\n\nlet value = 1\n";

async fn cancelling_gateway() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("gateway address");
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let _ = serve(socket).await;
            });
        }
    });
    address
}

async fn serve(socket: TcpStream) -> anyhow::Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    while let Some(message) = framed.next().await {
        match message? {
            WireMessage::SyncProbeRequest(request) => {
                let missing = request
                    .files
                    .iter()
                    .map(|entry| entry.relative_path.clone())
                    .collect();
                framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: request.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing,
                    }))
                    .await?;
            }
            WireMessage::SyncRequest(request) => {
                framed
                    .send(WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: request.client_workspace_root,
                        files_updated: request.files.len(),
                        files_deleted: 0,
                        bytes_transferred: 0,
                        duration_ms: 1,
                        workspace_was_fresh: false,
                        stale_paths: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::HandshakeRequest(request) => {
                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version: PROTOCOL_VERSION,
                        server_pid: std::process::id(),
                        session_id: 1,
                        server_workspace_root: request.client_workspace_root,
                        detected_engine: "swift".to_string(),
                        stale_paths: Vec::new(),
                        engine_age_ms: None,
                        index_gated: false,
                        capabilities: None,
                    }))
                    .await?;
            }
            WireMessage::LspPayload(payload) => {
                let Ok(request) = serde_json::from_str::<serde_json::Value>(&payload) else {
                    continue;
                };
                let Some(id) = request.get("id").cloned() else {
                    continue;
                };
                let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("");
                let reply = match method {
                    "initialize" => serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": { "capabilities": { "definitionProvider": true } }
                    }),
                    "textDocument/definition" => serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32800, "message": "request cancelled by client" }
                    }),
                    _ => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": null }),
                };
                framed
                    .send(WireMessage::LspPayload(reply.to_string()))
                    .await?;
            }
            WireMessage::Disconnect { .. } => break,
            _ => {}
        }
    }
    Ok(())
}

fn text_of(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn workspace() -> Workspace {
    Workspace::new(&[
        ("Package.swift", "// swift-tools-version: 5.9\n"),
        ("Sources/App/TerminalController.swift", SOURCE),
    ])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_swift_definition_falls_back_to_the_local_outline() {
    let ws = workspace();
    let remote = cancelling_gateway().await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_definition",
        serde_json::json!({
            "path": "Sources/App/TerminalController.swift",
            "line": 2,
            "character": 15
        }),
    )
    .await
    .expect("a declaration in the file's own outline is answered despite the cancellation");
    let text = text_of(&result);
    assert!(!result.is_error, "{text}");
    assert!(text.contains("syntax-only local Swift outline"), "{text}");
    assert!(text.contains("request cancelled by client"), "{text}");
    assert!(text.contains("print(\"ok\")"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_swift_lookup_off_a_declaration_still_reports_the_failure() {
    let ws = workspace();
    let remote = cancelling_gateway().await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_definition",
        serde_json::json!({
            "path": "Sources/App/TerminalController.swift",
            "line": 2,
            "character": 24
        }),
    )
    .await;
    let text = match result {
        Ok(result) => {
            assert!(result.is_error, "{}", text_of(&result));
            text_of(&result)
        }
        Err(error) => format!("{error:#}"),
    };
    assert!(text.contains("request cancelled by client"), "{text}");
    assert!(!text.contains("syntax-only"), "{text}");
}
