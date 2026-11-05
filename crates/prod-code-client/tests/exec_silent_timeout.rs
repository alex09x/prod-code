/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A command that ran to its deadline without printing anything is reported with what usually
//! makes it so, a wait for input the node cannot give (#1892).

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::cluster::PlacementStore;
use prod_code_protocol::{
    ExecChunk, ExecExit, ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage,
};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

/// A node whose one command runs to its deadline; it printed `output` first, if any.
async fn node(output: Option<&'static [u8]>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        while let Some(Ok(message)) = framed.next().await {
            let replies = match message {
                WireMessage::SyncProbeRequest(_) => {
                    vec![WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: "/remote/copy".into(),
                        seeded: false,
                        files_deleted: 0,
                        missing: vec![],
                    })]
                }
                WireMessage::SyncRequest(_) => vec![WireMessage::SyncResponse(SyncResponse {
                    server_workspace_root: "/remote/copy".into(),
                    files_updated: 0,
                    files_deleted: 0,
                    bytes_transferred: 0,
                    duration_ms: 0,
                    workspace_was_fresh: false,
                    stale_paths: vec![],
                })],
                WireMessage::ExecRequest(_) => {
                    let mut replies = Vec::new();
                    if let Some(bytes) = output {
                        replies.push(WireMessage::ExecChunk(ExecChunk {
                            stderr: false,
                            data: Some(bytes.to_vec()),
                        }));
                    }
                    replies.push(WireMessage::ExecExit(ExecExit {
                        exit_code: None,
                        duration_ms: 2_000,
                        server_workspace_root: "/remote/copy".into(),
                        timed_out: true,
                        error: None,
                        usage: None,
                        platform: None,
                    }));
                    replies
                }
                WireMessage::Disconnect { .. } => break,
                _ => continue,
            };
            for reply in replies {
                if framed.send(reply).await.is_err() {
                    return;
                }
            }
        }
    });
    (addr, task)
}

async fn run(output: Option<&'static [u8]>) -> String {
    let root = tempfile::tempdir().unwrap();
    let store_dir = tempfile::tempdir().unwrap();
    let placement = PlacementStore::at(store_dir.path().join("placement.json"));
    let (addr, server) = node(output).await;
    let result = prod_code_mcp::tools::execute_tool_with(
        placement,
        addr,
        root.path(),
        "code_exec",
        serde_json::json!({"argv": ["tool", "whoami"], "timeout_secs": 1}),
    )
    .await
    .unwrap();
    server.abort();
    assert!(result.is_error, "a timeout is an error result");
    serde_json::to_string(&result).unwrap()
}

#[tokio::test]
async fn a_silent_timeout_says_what_usually_makes_a_command_silent() {
    let text = run(None).await;
    assert!(text.contains("timed out"), "{text}");
    assert!(text.contains("standard input closed"), "{text}");
    assert!(text.contains("non-interactive"), "{text}");
}

#[tokio::test]
async fn a_timeout_after_some_output_is_not_blamed_on_a_wait_for_input() {
    let text = run(Some(b"working...\n")).await;
    assert!(text.contains("timed out"), "{text}");
    assert!(text.contains("working..."), "{text}");
    assert!(!text.contains("standard input closed"), "{text}");
}
