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
use prod_code_protocol::{ProdCodeCodec, ReadFileResponse, StatusResponse, WireMessage};
use serde_json::json;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio_util::codec::Framed;

struct Gateway {
    addr: SocketAddr,
    other_requests: Arc<AtomicUsize>,
    status_requests: Arc<AtomicUsize>,
    source_reads: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Gateway {
    async fn rust_only() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let other_requests = Arc::new(AtomicUsize::new(0));
        let seen = other_requests.clone();
        let status_requests = Arc::new(AtomicUsize::new(0));
        let statuses = status_requests.clone();
        let source_reads = Arc::new(AtomicUsize::new(0));
        let reads = source_reads.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let seen = seen.clone();
                let statuses = statuses.clone();
                let reads = reads.clone();
                tokio::spawn(async move {
                    let mut framed = Framed::new(socket, ProdCodeCodec::new());
                    while let Some(Ok(message)) = framed.next().await {
                        match message {
                            WireMessage::StatusRequest => {
                                statuses.fetch_add(1, Ordering::SeqCst);
                                let status = StatusResponse {
                                    detected_engines: vec!["rust".into(), "python".into()],
                                    platform: Some("linux x86_64".into()),
                                    ..Default::default()
                                };
                                if framed
                                    .send(WireMessage::StatusResponse(status))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            WireMessage::ReadFileRequest(request) => {
                                reads.fetch_add(1, Ordering::SeqCst);
                                let response = ReadFileResponse {
                                    path: request.path,
                                    content: Some(b"# SDK source\n".to_vec()),
                                    truncated: false,
                                    is_executable: None,
                                    error: None,
                                };
                                if framed
                                    .send(WireMessage::ReadFileResponse(response))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            _ => {
                                seen.fetch_add(1, Ordering::SeqCst);
                                break;
                            }
                        }
                    }
                });
            }
        });
        Self {
            addr,
            other_requests,
            status_requests,
            source_reads,
            task,
        }
    }
}

fn checkout() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"source-fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir(root.path().join("scripts")).unwrap();
    let script: String = (1..=120).map(|line| format!("# line {line}\n")).collect();
    std::fs::write(root.path().join("scripts/check.sh"), script).unwrap();
    root
}

fn assert_snippet(text: &str) {
    assert!(text.contains("# line 100"), "{text}");
    assert!(text.contains("# line 98"), "{text}");
    assert!(text.contains("# line 102"), "{text}");
    assert!(!text.contains("# line 97"), "{text}");
    assert!(!text.contains("# line 103"), "{text}");
}

#[tokio::test]
async fn issue_1818_mcp_source_reads_shell_without_analyzer_routing() {
    let root = checkout();
    let gateway = Gateway::rust_only().await;
    prod_code_mcp::cluster::set_routing_with_policy(
        vec![gateway.addr],
        prod_code_mcp::sync::workspace_identity(root.path()).name,
        prod_code_mcp::cluster::NodePolicy::default(),
    );
    for key in ["path", "file", "file_path"] {
        let mut args = json!({ "line": 100, "context": 2 });
        args[key] = json!("scripts/check.sh");
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            prod_code_mcp::tools::execute_tool(gateway.addr, root.path(), "code_source", args),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!result.is_error, "{result:?}");
        assert_snippet(&serde_json::to_string(&result).unwrap());
    }
    assert_eq!(gateway.source_reads.load(Ordering::SeqCst), 0);
    let statuses = gateway.status_requests.load(Ordering::SeqCst);
    let sdk = prod_code_mcp::tools::execute_tool(
        gateway.addr,
        root.path(),
        "code_source",
        json!({ "path": "/sdk/source-fixture.py", "line": 1, "context": 0 }),
    )
    .await
    .unwrap();
    assert!(!sdk.is_error, "{sdk:?}");
    assert!(
        serde_json::to_string(&sdk)
            .unwrap()
            .contains("# SDK source")
    );
    assert!(
        gateway.status_requests.load(Ordering::SeqCst) > statuses,
        "external SDK sources retain their existing language-affinity routing"
    );
    assert_eq!(gateway.source_reads.load(Ordering::SeqCst), 1);
    let escaped = prod_code_mcp::tools::execute_tool(
        gateway.addr,
        root.path(),
        "code_source",
        json!({ "path": "../outside.sh", "line": 1, "context": 2 }),
    )
    .await;
    assert!(
        escaped.is_err(),
        "relative paths may not leave the checkout"
    );
    assert_eq!(
        gateway.other_requests.load(Ordering::SeqCst),
        0,
        "a local bounded source read must not start an analyzer or sync session"
    );
}

async fn cli(root: &Path, addr: SocketAddr) -> std::process::Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"));
    command
        .current_dir(root)
        .args([
            "--remote",
            &addr.to_string(),
            "source",
            "scripts/check.sh",
            "--line",
            "100",
            "--context",
            "2",
        ])
        .env_remove("HOME")
        .env_remove(prod_code_mcp::cluster::PIN_ENV)
        .env_remove(prod_code_mcp::cluster::PIN_RESUMED_ENV)
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(8), command.output())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn issue_1818_cli_source_does_not_require_a_shell_gateway() {
    let root = checkout();
    let gateway = Gateway::rust_only().await;
    let result = cli(root.path(), gateway.addr).await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_snippet(&String::from_utf8_lossy(&result.stdout));
    assert_eq!(gateway.other_requests.load(Ordering::SeqCst), 0);
}
