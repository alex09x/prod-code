/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::execute_tool;
use crate::protocol::McpContentItem;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeResponse, PROTOCOL_VERSION, ProdCodeCodec, SyncProbeResponse, SyncResponse,
    WireMessage,
};
use prod_code_testkit::{Workspace, answers};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

const PACKAGE: &str = "// swift-tools-version: 5.9\n";

async fn reference_gateway(
    first: std::path::PathBuf,
    second: std::path::PathBuf,
    query_started: Arc<AtomicBool>,
) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let first = first.clone();
            let second = second.clone();
            let query_started = Arc::clone(&query_started);
            tokio::spawn(async move {
                let _ = serve_reference_gateway(socket, first, second, query_started).await;
            });
        }
    });
    address
}

async fn serve_reference_gateway(
    socket: TcpStream,
    first: std::path::PathBuf,
    second: std::path::PathBuf,
    query_started: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    while let Some(message) = framed.next().await {
        match message? {
            WireMessage::SyncProbeRequest(request) => {
                let missing = request
                    .files
                    .iter()
                    .map(|file| file.relative_path.clone())
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
                    .filter_map(|file| file.content.as_ref())
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
                        detected_engine: "swift".to_string(),
                        stale_paths: Vec::new(),
                        engine_age_ms: None,
                        index_gated: false,
                        capabilities: None,
                    }))
                    .await?;
            }
            WireMessage::LspPayload(payload) => {
                let request: serde_json::Value = serde_json::from_str(&payload)?;
                let Some(id) = request.get("id").cloned() else {
                    continue;
                };
                let result = match request.get("method").and_then(|value| value.as_str()) {
                    Some("initialize") => serde_json::json!({
                        "capabilities": {
                            "workspaceSymbolProvider": true,
                            "referencesProvider": true
                        }
                    }),
                    Some("workspace/symbol") => serde_json::json!([
                        answers::symbol("handleEvent", 2, &first, 2, 10),
                        answers::symbol("handleEvent", 2, &second, 2, 10),
                    ]),
                    Some("textDocument/references") => {
                        query_started.store(true, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                        serde_json::Value::Null
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

pub(super) async fn run() {
    let ws = Workspace::new(&[
        ("Package.swift", PACKAGE),
        (
            "Sources/A.swift",
            "struct Alpha {\n    func handleEvent() {}\n}\n",
        ),
        (
            "Sources/B.swift",
            "struct Beta {\n    func handleEvent() {}\n}\n",
        ),
    ]);
    let first = ws.path("Sources/A.swift");
    let second = ws.path("Sources/B.swift");
    let query_started = Arc::new(AtomicBool::new(false));
    let addr = reference_gateway(first, second, Arc::clone(&query_started)).await;
    let root = ws.root();

    let result = execute_tool(
        addr,
        &root,
        "code_references",
        serde_json::json!({ "symbol": "handleEvent", "budget_ms": 100 }),
    )
    .await
    .unwrap();

    let McpContentItem::Text { text } = &result.content[0];
    assert!(
        query_started.load(Ordering::SeqCst),
        "reference request did not start"
    );
    assert!(
        text.contains("`handleEvent` matched 2 candidates:"),
        "{text}"
    );
    assert!(text.contains("=== [1/2]"), "{text}");
    assert!(
        text.contains("Partial result: shared server time budget")
            || text.contains("returning partial multi-candidate result"),
        "{text}"
    );
    assert!(!text.contains("=== [2/2]"), "{text}");
}
