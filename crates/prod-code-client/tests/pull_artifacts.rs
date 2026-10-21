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
use prod_code_protocol::{
    ExecExit, PULL_CHUNK_BYTES, ProdCodeCodec, PullFileResponse, ReadFileResponse,
    SyncProbeResponse, SyncResponse, WireMessage,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

const COPY: &str = "/remote/owned-copy";

/// One exec connection followed by one artifact connection, with real framed binary data.
async fn artifact_node(size: u64, missing: bool) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let node = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        for _ in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut framed = Framed::new(stream, ProdCodeCodec::new());
            while let Some(Ok(message)) = framed.next().await {
                let reply = match message {
                    WireMessage::Auth(_) => continue,
                    WireMessage::SyncProbeRequest(_) => {
                        WireMessage::SyncProbeResponse(SyncProbeResponse {
                            server_workspace_root: COPY.into(),
                            seeded: false,
                            files_deleted: 0,
                            missing: vec![],
                        })
                    }
                    WireMessage::SyncRequest(_) => WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: COPY.into(),
                        files_updated: 0,
                        files_deleted: 0,
                        bytes_transferred: 0,
                        duration_ms: 0,
                        workspace_was_fresh: false,
                        stale_paths: vec![],
                    }),
                    WireMessage::ExecRequest(_) => WireMessage::ExecExit(ExecExit {
                        exit_code: Some(0),
                        duration_ms: 1,
                        server_workspace_root: COPY.into(),
                        timed_out: false,
                        error: None,
                        usage: None,
                        platform: None,
                    }),
                    WireMessage::PullFileRequest(req) => {
                        assert_eq!(req.path, "target/release/program");
                        let length = (size - req.offset).min(PULL_CHUNK_BYTES) as usize;
                        WireMessage::PullFileResponse(PullFileResponse {
                            server_workspace_root: COPY.into(),
                            offset: req.offset,
                            total_bytes: size,
                            file: ReadFileResponse {
                                path: req.path,
                                content: (!missing).then(|| vec![0xa5; length]),
                                truncated: req.offset + (length as u64) < size,
                                is_executable: Some(true),
                                error: missing
                                    .then(|| "file not found in this checkout's copy".into()),
                            },
                        })
                    }
                    WireMessage::Disconnect { .. } => break,
                    // An analyzer handshake is the old broken path. Close it promptly.
                    WireMessage::HandshakeRequest(_) => break,
                    other => panic!("unexpected message: {other:?}"),
                };
                if framed.send(reply).await.is_err() {
                    break;
                }
            }
        }
    });
    (node, task)
}

async fn exec(root: &std::path::Path, node: SocketAddr) {
    let result = prod_code_mcp::tools::execute_tool(
        node,
        root,
        "code_exec",
        serde_json::json!({"argv": ["build-fixture"]}),
    )
    .await
    .unwrap();
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains(&format!("node {node}")), "{text}");
    assert!(text.contains(&format!("copy {COPY}")), "{text}");
}

#[tokio::test]
async fn pull_uses_exec_node_for_large_executable_build_output() {
    let root = tempfile::tempdir().unwrap();
    let size = 33 * 1024 * 1024 + 19;
    let (node, server) = artifact_node(size, false).await;
    exec(root.path(), node).await;
    // Analyzer placement is independent; default pull must still hit the exec node.
    let other: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let identity = prod_code_mcp::sync::workspace_identity(root.path());
    prod_code_mcp::cluster::remember_placement(&identity.name, other);
    let files = prod_code_mcp::sync::pull_remote_files(
        other,
        root.path(),
        &[PathBuf::from("target/release/program")],
    )
    .await;
    if files.is_err() {
        server.abort();
    }
    let _ = server.await;
    assert_eq!(files.unwrap(), ["target/release/program"]);
    let artifact = root.path().join("target/release/program");
    let bytes = std::fs::read(&artifact).unwrap();
    assert_eq!(bytes.len() as u64, size);
    assert!(bytes.iter().all(|b| *b == 0xa5));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            std::fs::metadata(artifact).unwrap().permissions().mode() & 0o111,
            0
        );
    }
}

#[tokio::test]
async fn pull_missing_file_names_actual_node_copy_and_exec_guidance() {
    let root = tempfile::tempdir().unwrap();
    let (node, server) = artifact_node(0, true).await;
    exec(root.path(), node).await;
    let error = prod_code_mcp::sync::pull_remote_files(
        node,
        root.path(),
        &[PathBuf::from("target/release/program")],
    )
    .await
    .unwrap_err()
    .to_string();
    server.await.unwrap();
    for expected in [
        node.to_string(),
        COPY.into(),
        "not found".into(),
        "prod-code exec".into(),
        format!("--remote {node} pull"),
    ] {
        assert!(error.contains(&expected), "missing {expected}: {error}");
    }
}

#[tokio::test]
async fn pull_mcp_uses_last_exec_node_and_reports_copy() {
    let root = tempfile::tempdir().unwrap();
    let (node, server) = artifact_node(7, false).await;
    exec(root.path(), node).await;
    let result = prod_code_mcp::tools::execute_tool(
        "127.0.0.1:1".parse().unwrap(),
        root.path(),
        "code_pull",
        serde_json::json!({"files": ["target/release/program"]}),
    )
    .await;
    if result.is_err() {
        server.abort();
    }
    let _ = server.await;
    let text = serde_json::to_string(&result.unwrap()).unwrap();
    assert!(text.contains(&node.to_string()), "{text}");
    assert!(text.contains(COPY), "{text}");
    assert_eq!(
        std::fs::read(root.path().join("target/release/program")).unwrap(),
        vec![0xa5; 7]
    );
}

#[tokio::test]
async fn pull_mcp_explicit_remote_overrides_exec_record() {
    let root = tempfile::tempdir().unwrap();
    let (node, server) = artifact_node(7, false).await;
    exec(root.path(), node).await;
    prod_code_mcp::exec::remember_exec_node(root.path(), "127.0.0.1:1".parse().unwrap());
    let result = prod_code_mcp::tools::execute_tool(
        "127.0.0.1:1".parse().unwrap(),
        root.path(),
        "code_pull",
        serde_json::json!({"files": ["target/release/program"], "remote": node.to_string()}),
    )
    .await;
    if result.is_err() {
        server.abort();
    }
    let _ = server.await;
    let text = serde_json::to_string(&result.unwrap()).unwrap();
    assert!(text.contains(&node.to_string()), "{text}");
}
