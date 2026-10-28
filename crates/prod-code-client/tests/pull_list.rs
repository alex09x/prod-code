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
    ArtifactEntry, ArtifactListing, ProdCodeCodec, PullFileResponse, ReadFileResponse,
    StatusResponse, WireMessage,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::codec::Framed;

#[test]
fn pull_help_offers_a_read_only_listing() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .args(["pull", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("--list"), "{text}");
}

async fn node(
    old: bool,
) -> (
    std::net::SocketAddr,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let syncs = Arc::new(AtomicUsize::new(0));
    let seen = syncs.clone();
    let task = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(message)) = framed.next().await {
                    let response = match message {
                        WireMessage::StatusRequest => WireMessage::StatusResponse(StatusResponse {
                            detected_engines: vec!["rust".into()],
                            cpu_count: Some(4),
                            ..Default::default()
                        }),
                        WireMessage::SyncRequest(_) | WireMessage::SyncProbeRequest(_) => {
                            seen.fetch_add(1, Ordering::SeqCst);
                            break;
                        }
                        WireMessage::PullFileRequest(req) => {
                            assert_eq!(req.path, "evidence/run");
                            assert!(req.list_directory);
                            let bytes = serde_json::to_vec(&ArtifactListing {
                                entries: vec![ArtifactEntry {
                                    name: "result.json".into(),
                                    kind: "file".into(),
                                }],
                                truncated: false,
                            })
                            .unwrap();
                            WireMessage::PullFileResponse(PullFileResponse {
                                server_workspace_root: "/remote/owned-copy".into(),
                                offset: 0,
                                total_bytes: if old { 0 } else { bytes.len() as u64 },
                                file: ReadFileResponse {
                                    path: req.path,
                                    content: (!old).then_some(bytes),
                                    truncated: false,
                                    is_executable: None,
                                    error: old.then(|| "artifact is not a regular file".into()),
                                },
                            })
                        }
                        WireMessage::Disconnect { .. } => break,
                        WireMessage::Auth(_) => continue,
                        other => panic!("unexpected message: {other:?}"),
                    };
                    if framed.send(response).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    (address, syncs, task)
}

#[tokio::test]
async fn cli_and_mcp_list_without_sync_or_local_writes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    let (address, syncs, server) = node(false).await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(address.to_string())
        .args(["pull", "--list", "evidence/run"])
        .current_dir(root.path())
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("result.json") && stdout.contains("/remote/owned-copy"),
        "{stdout}"
    );
    let result = prod_code_mcp::tools::execute_tool(
        address,
        root.path(),
        "code_pull",
        serde_json::json!({"files":["evidence/run"],"list":true,"remote":address.to_string()}),
    )
    .await
    .unwrap();
    assert!(!result.is_error);
    let result = serde_json::to_string(&result).unwrap();
    assert!(result.contains("result.json"), "{result}");
    prod_code_mcp::exec::remember_exec_node(root.path(), address);
    let fallback = "127.0.0.1:1".parse().unwrap();
    let result = prod_code_mcp::tools::execute_tool(
        fallback,
        root.path(),
        "code_pull",
        serde_json::json!({"files":["evidence/run"],"list":true}),
    )
    .await
    .unwrap();
    assert!(!result.is_error);
    assert!(
        serde_json::to_string(&result)
            .unwrap()
            .contains("result.json")
    );
    assert_eq!(syncs.load(Ordering::SeqCst), 0);
    assert!(!root.path().join("evidence").exists());
    let multiple = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(address.to_string())
        .args(["pull", "--list", "evidence/run", "another"])
        .current_dir(root.path())
        .output()
        .await
        .unwrap();
    assert!(!multiple.status.success());
    assert!(String::from_utf8_lossy(&multiple.stderr).contains("exactly one directory"));
    assert_eq!(syncs.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn old_gateway_refuses_listing_without_local_writes() {
    let root = tempfile::tempdir().unwrap();
    let (address, syncs, server) = node(true).await;
    let error = prod_code_mcp::sync::list_remote_artifacts_from(
        address,
        root.path(),
        std::path::Path::new("evidence/run"),
    )
    .await
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("older gateways"), "{error}");
    assert_eq!(syncs.load(Ordering::SeqCst), 0);
    assert!(!root.path().join("evidence").exists());
    server.abort();
}
