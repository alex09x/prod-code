/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::protocol::McpContentItem;
use crate::tools::execute_tool;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeResponse, PROTOCOL_VERSION, ProdCodeCodec, SyncProbeResponse, SyncResponse,
    WireMessage,
};
use prod_code_testkit::{Workspace, answers};
use std::path::PathBuf;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

/// A C root whose server lists its own `applyTheme`, and a Swift package whose server, with no
/// build index, lists nothing for `workspace/symbol` but outlines its files.
async fn gateway(c_file: PathBuf, swift_file: PathBuf) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let (c_file, swift_file) = (c_file.clone(), swift_file.clone());
            tokio::spawn(async move {
                let _ = serve(socket, c_file, swift_file).await;
            });
        }
    });
    address
}

async fn serve(socket: TcpStream, c_file: PathBuf, swift_file: PathBuf) -> anyhow::Result<()> {
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
                        detected_engine: "cpp".to_string(),
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
                let uri = request["params"]["textDocument"]["uri"].as_str();
                let result = match request.get("method").and_then(|value| value.as_str()) {
                    Some("initialize") => serde_json::json!({
                        "capabilities": { "workspaceSymbolProvider": true }
                    }),
                    Some("workspace/symbol") => {
                        serde_json::json!([answers::symbol("applyTheme", 12, &c_file, 1, 6)])
                    }
                    Some("textDocument/documentSymbol")
                        if uri == Some(answers::uri(&swift_file).as_str()) =>
                    {
                        serde_json::json!([answers::nested(
                            answers::document_symbol("Demo.SurfaceView", 3, 1, 3, 11),
                            vec![answers::document_symbol("applyTheme(_:)", 6, 2, 2, 10)],
                        )])
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

/// `code_symbols applyTheme` in a C checkout with a Swift package inside: the root's function
/// used to end the search, and the Swift method of that name, in an extension, went unlisted.
#[tokio::test]
async fn a_root_match_does_not_hide_a_swift_declaration_of_that_name() {
    let ws = Workspace::new(&[
        (
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)\nproject(demo C)\n",
        ),
        ("src/main.c", "void applyTheme(int c) { (void)c; }\n"),
        ("swift/Package.swift", "// swift-tools-version: 5.9\n"),
        (
            "swift/Sources/App/Demo+Actions.swift",
            "extension Demo.SurfaceView {\n    func applyTheme(_ c: Int) {}\n}\n",
        ),
    ]);
    let c_file = ws.path("src/main.c");
    let swift_file = ws.path("swift/Sources/App/Demo+Actions.swift");
    let addr = gateway(c_file, swift_file).await;

    let result = execute_tool(
        addr,
        &ws.root(),
        "code_symbols",
        serde_json::json!({ "query": "applyTheme" }),
    )
    .await
    .unwrap();

    let McpContentItem::Text { text } = &result.content[0];
    assert!(text.contains("src/main.c:1:6"), "{text}");
    assert!(
        text.contains("swift/Sources/App/Demo+Actions.swift:2:10"),
        "{text}"
    );
}
