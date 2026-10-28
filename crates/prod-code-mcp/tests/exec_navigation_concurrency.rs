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
use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_protocol::{
    ExecExit, ProdCodeCodec, StatusResponse, SyncProbeResponse, SyncResponse, WireMessage,
};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};
use tokio_util::codec::Framed;

#[tokio::test]
async fn definition_completes_while_exec_is_running() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='demo'\nversion='0.1.0'\nedition='2024'\n",
        ),
        ("src/lib.rs", "pub fn target() {}\n"),
    ]);
    ws.commit();
    let path = ws.path("src/lib.rs");
    let query_gateway = ScriptedGateway::start(move |method, _| match method {
        "textDocument/definition" => answers::locations(&path, &[(1, 8)]),
        _ => serde_json::Value::Null,
    })
    .await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let exec_addr = listener.local_addr().unwrap();
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let started = Arc::new(Mutex::new(Some(started_tx)));
    let release = Arc::new(Mutex::new(Some(release_rx)));
    let server = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(message)) = framed.next().await {
                    let response = match message {
                        WireMessage::StatusRequest => WireMessage::StatusResponse(StatusResponse {
                            detected_engines: vec!["rust".into()],
                            cpu_count: Some(4),
                            ..Default::default()
                        }),
                        WireMessage::SyncProbeRequest(req) => {
                            WireMessage::SyncProbeResponse(SyncProbeResponse {
                                server_workspace_root: req.client_workspace_root,
                                seeded: false,
                                files_deleted: 0,
                                missing: vec![],
                            })
                        }
                        WireMessage::SyncRequest(req) => WireMessage::SyncResponse(SyncResponse {
                            server_workspace_root: req.client_workspace_root,
                            files_updated: 0,
                            files_deleted: 0,
                            bytes_transferred: 0,
                            duration_ms: 0,
                            workspace_was_fresh: false,
                            stale_paths: vec![],
                        }),
                        WireMessage::ExecRequest(req) => {
                            if let Some(tx) = started.lock().await.take() {
                                let _ = tx.send(());
                            }
                            if let Some(rx) = release.lock().await.take() {
                                let _ = rx.await;
                            }
                            WireMessage::ExecExit(ExecExit {
                                exit_code: Some(0),
                                duration_ms: 1,
                                server_workspace_root: req.client_workspace_root,
                                timed_out: false,
                                error: None,
                                usage: None,
                                platform: None,
                            })
                        }
                        WireMessage::Disconnect { .. } => break,
                        _ => continue,
                    };
                    if framed.send(response).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    let root = ws.root().to_path_buf();
    let exec_root = root.clone();
    let exec = tokio::spawn(async move {
        execute_tool(
            exec_addr,
            &exec_root,
            "code_exec",
            serde_json::json!({"argv": ["sleep", "1"]}),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), started_rx)
        .await
        .unwrap()
        .unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        execute_tool(
            query_gateway.addr(),
            &root,
            "code_definition",
            serde_json::json!({"path":"src/lib.rs","line":1,"character":8,"body":false}),
        ),
    )
    .await;
    let writer_root = root.clone();
    let mut second_writer = tokio::spawn(async move {
        execute_tool(
            exec_addr,
            &writer_root,
            "code_exec",
            serde_json::json!({"argv": []}),
        )
        .await
    });
    let writer_waited =
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut second_writer)
            .await
            .is_err();
    let rename_root = root.clone();
    let query_addr = query_gateway.addr();
    let mut rename = tokio::spawn(async move {
        execute_tool(
            query_addr,
            &rename_root,
            "code_rename",
            serde_json::json!({"path":"src/lib.rs","line":1,"character":8}),
        )
        .await
    });
    let rename_waited = tokio::time::timeout(std::time::Duration::from_millis(200), &mut rename)
        .await
        .is_err();
    let _ = release_tx.send(());
    let exec_result = exec.await.unwrap();
    let second_result = second_writer.await.unwrap();
    let rename_result = tokio::time::timeout(std::time::Duration::from_secs(10), rename).await;
    server.abort();
    let definition = result
        .expect("definition waited for the running exec")
        .expect("definition should succeed");
    assert!(!definition.is_error);
    let McpContentItem::Text { text } = &definition.content[0];
    assert!(text.contains("Definition:"), "{text}");
    assert!(
        writer_waited,
        "another writer must wait until exec finishes"
    );
    assert!(
        second_result.is_err(),
        "the empty command must be rejected after waiting"
    );
    assert!(
        rename_waited,
        "a mutating tool must wait until exec finishes"
    );
    assert!(
        rename_result
            .expect("rename must finish after exec")
            .unwrap()
            .is_err()
    );
    assert!(exec_result.is_ok(), "exec should finish");
}
