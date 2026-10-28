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
use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_mcp::tools::execute_tool;
use prod_code_protocol::{
    HandshakeResponse, PROTOCOL_VERSION, ProdCodeCodec, SyncProbeResponse, SyncResponse,
    WireMessage,
};
use prod_code_testkit::{Workspace, answers};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

const CARGO_TOML: &str =
    "[package]\nname = \"definition_budget_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str = "pub fn target() {}\n";

async fn slow_symbol_gateway(file: &std::path::Path, started: Arc<AtomicBool>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("gateway address");
    let file = file.to_path_buf();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let file = file.clone();
            let started = Arc::clone(&started);
            tokio::spawn(async move {
                let _ = serve_slow_symbol_query(socket, file, started).await;
            });
        }
    });
    address
}

async fn serve_slow_symbol_query(
    socket: TcpStream,
    file: std::path::PathBuf,
    started: Arc<AtomicBool>,
) -> anyhow::Result<()> {
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
                let bytes_transferred = request
                    .files
                    .iter()
                    .filter_map(|entry| entry.content.as_ref())
                    .map(Vec::len)
                    .sum();
                framed
                    .send(WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: request.client_workspace_root,
                        files_updated: request.files.len(),
                        files_deleted: 0,
                        bytes_transferred,
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
                        detected_engine: "rust".to_string(),
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
                let result = match method {
                    "initialize" => serde_json::json!({
                        "capabilities": {
                            "workspaceSymbolProvider": true,
                            "definitionProvider": true,
                            "referencesProvider": true
                        }
                    }),
                    "workspace/symbol" => {
                        started.store(true, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(1_500)).await;
                        serde_json::json!([answers::symbol("target", 12, &file, 1, 8)])
                    }
                    "textDocument/definition" => answers::locations(&file, &[(1, 8)]),
                    "textDocument/references" => {
                        started.store(true, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(1_500)).await;
                        serde_json::json!([])
                    }
                    _ => serde_json::Value::Null,
                };
                framed
                    .send(WireMessage::LspPayload(
                        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
                            .to_string(),
                    ))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn code_definition_returns_a_busy_response_inside_its_shared_budget() {
    let definition = prod_code_mcp::tools::list_tools()
        .into_iter()
        .find(|tool| tool.name == "code_definition")
        .expect("code_definition is advertised");
    assert!(
        definition.input_schema["properties"]
            .get("budget_ms")
            .is_some(),
        "code_definition must advertise its shared budget"
    );

    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", LIB)]);
    let file = ws.path("src/lib.rs");
    let query_started = Arc::new(AtomicBool::new(false));
    let started = Arc::clone(&query_started);
    let remote = slow_symbol_gateway(&file, started).await;

    let started_at = Instant::now();
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_definition",
        serde_json::json!({
            "path": "src/lib.rs",
            "symbol": "target",
            "body": false,
            "budget_ms": 500
        }),
    )
    .await;
    let elapsed = started_at.elapsed();
    let text = match result {
        Ok(result) => {
            assert!(result.is_error, "{result:?}");
            text_of(&result)
        }
        Err(error) => format!("{error:#}"),
    };

    assert!(
        query_started.load(Ordering::SeqCst),
        "symbol lookup did not start"
    );
    assert!(
        text.contains("code_definition did not finish within 500 ms"),
        "{text}"
    );
    assert!(
        elapsed < Duration::from_millis(1_250),
        "the shared deadline did not return before the 1.5s stalled query: {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn code_references_returns_a_busy_response_inside_its_shared_budget() {
    let references = prod_code_mcp::tools::list_tools()
        .into_iter()
        .find(|tool| tool.name == "code_references")
        .expect("code_references is advertised");
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", LIB)]);
    let file = ws.path("src/lib.rs");
    let query_started = Arc::new(AtomicBool::new(false));
    let started = Arc::clone(&query_started);
    let remote = slow_symbol_gateway(&file, started).await;

    let started_at = Instant::now();
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_references",
        serde_json::json!({
            "path": "src/lib.rs",
            "line": 1,
            "character": 8,
            "budget_ms": 500
        }),
    )
    .await;
    let elapsed = started_at.elapsed();
    let (is_error, text) = match result {
        Ok(result) => (result.is_error, text_of(&result)),
        Err(error) => (true, format!("{error:#}")),
    };

    assert!(
        query_started.load(Ordering::SeqCst),
        "references lookup did not start"
    );
    assert!(
        is_error && text.contains("code_references did not finish within 500 ms"),
        "is_error={is_error}; {text}"
    );
    assert!(
        elapsed < Duration::from_millis(1_250),
        "the reference deadline did not return before the 1.5s stalled query: {elapsed:?}"
    );
    assert!(
        references.input_schema["properties"]
            .get("budget_ms")
            .is_some(),
        "code_references must advertise its shared budget"
    );
}
