/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[allow(dead_code)]
mod lsp_reconnect_support;

use futures_util::{SinkExt, StreamExt};
use lsp_reconnect_support::{response, spawn_bridge};
use prod_code_client::editor_files::write_frame;
use prod_code_protocol::{
    HandshakeResponse, PROTOCOL_VERSION, ProdCodeCodec, SyncProbeResponse, SyncResponse,
    WireMessage,
};
use prod_code_testkit::Workspace;
use serde_json::json;
use std::time::Duration;
use tokio::io::BufReader;
use tokio_util::codec::Framed;

#[tokio::test]
async fn stale_handshake_files_are_reuploaded_before_initialize() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn restored() {}\n")]);
    let cache = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        let mut uploaded = false;
        while let Some(Ok(message)) = framed.next().await {
            match message {
                WireMessage::SyncProbeRequest(req) => framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing: vec![],
                    }))
                    .await
                    .unwrap(),
                WireMessage::SyncRequest(req) => {
                    framed
                        .send(WireMessage::SyncResponse(SyncResponse {
                            server_workspace_root: req.client_workspace_root,
                            files_updated: 0,
                            files_deleted: 0,
                            bytes_transferred: 0,
                            duration_ms: 0,
                            workspace_was_fresh: false,
                            stale_paths: vec![],
                        }))
                        .await
                        .unwrap();
                }
                WireMessage::HandshakeRequest(req) => {
                    framed
                        .send(WireMessage::HandshakeResponse(HandshakeResponse {
                            protocol_version: PROTOCOL_VERSION,
                            server_pid: 1,
                            session_id: 1,
                            server_workspace_root: req.client_workspace_root,
                            detected_engine: "rust".into(),
                            stale_paths: vec!["src/lib.rs".into()],
                            engine_age_ms: None,
                            index_gated: false,
                            capabilities: None,
                        }))
                        .await
                        .unwrap();
                    // Native editor proxy ignores sync frames on the LSP connection.
                    let (sync_socket, _) =
                        tokio::time::timeout(Duration::from_secs(3), listener.accept())
                            .await
                            .expect("a separate sync connection is required")
                            .unwrap();
                    let mut sync = Framed::new(sync_socket, ProdCodeCodec::new());
                    while let Some(Ok(message)) = sync.next().await {
                        match message {
                            WireMessage::SyncProbeRequest(req) => sync
                                .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    seeded: false,
                                    files_deleted: 0,
                                    missing: vec![],
                                }))
                                .await
                                .unwrap(),
                            WireMessage::SyncRequest(req) => {
                                uploaded |= req.files.iter().any(|file| {
                                    file.relative_path == "src/lib.rs" && file.content.is_some()
                                });
                                sync.send(WireMessage::SyncResponse(SyncResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    files_updated: 0,
                                    files_deleted: 0,
                                    bytes_transferred: 0,
                                    duration_ms: 0,
                                    workspace_was_fresh: false,
                                    stale_paths: vec![],
                                }))
                                .await
                                .unwrap();
                            }
                            WireMessage::Disconnect { .. } => break,
                            other => panic!("unexpected sync frame: {other:?}"),
                        }
                    }
                }
                WireMessage::LspPayload(raw) => {
                    let request: serde_json::Value = serde_json::from_str(&raw).unwrap();
                    if request["method"] == "initialize" {
                        framed
                            .send(WireMessage::LspPayload(
                                json!({"id":request["id"],"result":{"capabilities":{}}})
                                    .to_string(),
                            ))
                            .await
                            .unwrap();
                        return uploaded;
                    }
                }
                _ => {}
            }
        }
        false
    });
    let mut child = spawn_bridge(&ws, cache.path(), addr, None);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}})
            .to_string(),
    )
    .await
    .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(10), response(&mut stdout, 1))
        .await
        .expect("initialize response");
    assert!(
        peer.await.unwrap(),
        "stale file must be uploaded before initialize"
    );
}
