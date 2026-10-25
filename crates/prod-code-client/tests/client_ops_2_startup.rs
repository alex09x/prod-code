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
use prod_code_client::editor_files::{read_frame, write_frame};
use prod_code_protocol::{
    HandshakeResponse, PROTOCOL_VERSION, ProdCodeCodec, SyncProbeResponse, SyncResponse,
    WireMessage,
};
use prod_code_testkit::Workspace;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::io::BufReader;
use tokio_util::codec::Framed;

#[tokio::test]
async fn initialized_after_startup_reconnect_is_forwarded_once() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn sample() {}\n")]);
    let cache = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (initialized_tx, initialized_rx) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let mut initialized_tx = Some(initialized_tx);
        let mut seen = vec![];
        for connection in 0..2 {
            let (socket, _) = listener.accept().await.unwrap();
            let mut framed = Framed::new(socket, ProdCodeCodec::new());
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
                    WireMessage::SyncRequest(req) => framed
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
                        .unwrap(),
                    WireMessage::HandshakeRequest(req) => framed
                        .send(WireMessage::HandshakeResponse(HandshakeResponse {
                            protocol_version: PROTOCOL_VERSION,
                            server_pid: 1,
                            session_id: connection + 1,
                            server_workspace_root: req.client_workspace_root,
                            detected_engine: "rust".into(),
                            stale_paths: vec![],
                            engine_age_ms: None,
                            index_gated: false,
                            capabilities: None,
                        }))
                        .await
                        .unwrap(),
                    WireMessage::LspPayload(raw) => {
                        let value: Value = serde_json::from_str(&raw).unwrap();
                        seen.push((connection, value.clone()));
                        if value["method"] == "initialize" {
                            if connection == 0 {
                                break;
                            }
                            framed.send(WireMessage::LspPayload(json!({"jsonrpc":"2.0","id":value["id"],"result":{"capabilities":{}}}).to_string())).await.unwrap();
                        }
                        if value["method"] == "initialized" {
                            initialized_tx.take().unwrap().send(()).unwrap();
                        }
                    }
                    WireMessage::Disconnect { .. } => break,
                    other => panic!("unexpected startup frame: {other:?}"),
                }
            }
        }
        seen
    });
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(addr.to_string())
        .args(["lsp", "--reconnect", "--watchdog-secs", "0"])
        .env("HOME", cache.path())
        .env("XDG_CACHE_HOME", cache.path())
        .env_remove("PROD_CODE_CLUSTER")
        .current_dir(ws.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let init = json!({"jsonrpc":"2.0","id":7,"method":"initialize","params":{}});
    write_frame(&mut stdin, &init.to_string()).await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(5), read_frame(&mut stdout))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&reply).unwrap(),
        json!({"jsonrpc":"2.0","id":7,"result":{"capabilities":{}}})
    );
    let initialized = json!({"jsonrpc":"2.0","method":"initialized","params":{}});
    write_frame(&mut stdin, &initialized.to_string())
        .await
        .unwrap();
    // Malformed cancellation parameters must be ignored without removing unrelated requests.
    let cancel = json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{}});
    write_frame(&mut stdin, &cancel.to_string()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), initialized_rx)
        .await
        .unwrap()
        .unwrap();
    drop(stdin);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
            .await
            .unwrap()
            .unwrap()
            .code(),
        Some(0)
    );
    assert_eq!(
        peer.await.unwrap(),
        vec![(0, init.clone()), (1, init), (1, initialized), (1, cancel)]
    );
}
