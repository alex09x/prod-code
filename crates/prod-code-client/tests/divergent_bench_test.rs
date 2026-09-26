//! Integration test: runs the divergent-worktree benchmark end to end against a lightweight
//! in-process mock gateway that speaks the real `WireMessage` wire protocol. This exercises the
//! full path (git worktree creation, mutation application, transparent dirty/untracked sync,
//! concurrent workers, latency accounting, and correctness verification) without depending on a
//! real prod-code-gateway or language server.

use futures_util::{SinkExt, StreamExt};
use prod_code_client::divergent_bench::{self, DivergentBenchConfig, MIN_WORKERS, WorkspaceMode};
use prod_code_protocol::{
    HandshakeResponse, ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

/// Starts a mock gateway that answers hover queries with whatever text the client itself sent
/// via `textDocument/didOpen` for that URI. This validates that the *client* correctly targets
/// each worktree's own mutated file content without any cross-worktree bleed, which is exactly
/// the invariant the divergent benchmark exists to check.
async fn spawn_mock_gateway() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock gateway should bind an ephemeral port");
    let addr = listener
        .local_addr()
        .expect("listener should have a local addr");

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(handle_connection(stream));
        }
    });

    addr
}

async fn handle_connection(stream: TcpStream) {
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let mut open_docs: HashMap<String, String> = HashMap::new();
    let mut next_session_id: u64 = 1;

    while let Some(Ok(msg)) = framed.next().await {
        match msg {
            WireMessage::HandshakeRequest(req) => {
                let session_id = next_session_id;
                next_session_id += 1;
                let resp = WireMessage::HandshakeResponse(HandshakeResponse {
                    protocol_version: req.protocol_version,
                    server_pid: std::process::id(),
                    session_id,
                    server_workspace_root: req.client_workspace_root.clone(),
                    detected_engine: "mock".to_string(),
                    stale_paths: Vec::new(),
                    engine_age_ms: None,
                    index_gated: false,
                });
                if framed.send(resp).await.is_err() {
                    break;
                }
            }
            WireMessage::SyncProbeRequest(req) => {
                // The mock holds nothing: every manifest entry is missing, nothing is seeded.
                let resp = WireMessage::SyncProbeResponse(SyncProbeResponse {
                    server_workspace_root: req.client_workspace_root.clone(),
                    seeded: false,
                    files_deleted: 0,
                    missing: req.files.into_iter().map(|f| f.relative_path).collect(),
                });
                if framed.send(resp).await.is_err() {
                    break;
                }
            }
            WireMessage::SyncRequest(req) => {
                let bytes_transferred = req
                    .files
                    .iter()
                    .map(|f| f.content.as_ref().map(|c| c.len()).unwrap_or(0))
                    .sum();
                let resp = WireMessage::SyncResponse(SyncResponse {
                    files_updated: req.files.len(),
                    files_deleted: 0,
                    bytes_transferred,
                    duration_ms: 0,
                    server_workspace_root: req.client_workspace_root.clone(),
                    workspace_was_fresh: false,
                    stale_paths: Vec::new(),
                });
                if framed.send(resp).await.is_err() {
                    break;
                }
            }
            WireMessage::LspPayload(payload) => {
                let Ok(val) = serde_json::from_str::<serde_json::Value>(&payload) else {
                    continue;
                };
                let method = val.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let id = val.get("id").cloned();

                match method {
                    "initialize" => {
                        if let Some(id) = id {
                            let resp = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": { "capabilities": {} }
                            });
                            if framed
                                .send(WireMessage::LspPayload(resp.to_string()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                    "textDocument/didOpen" => {
                        if let Some(text_doc) = val.pointer("/params/textDocument") {
                            let uri = text_doc
                                .get("uri")
                                .and_then(|u| u.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let text = text_doc
                                .get("text")
                                .and_then(|t| t.as_str())
                                .unwrap_or_default()
                                .to_string();
                            open_docs.insert(uri, text);
                        }
                    }
                    "textDocument/hover" => {
                        if let Some(id) = id {
                            let uri = val
                                .pointer("/params/textDocument/uri")
                                .and_then(|u| u.as_str())
                                .unwrap_or_default();
                            let text = open_docs.get(uri).cloned().unwrap_or_default();
                            let resp = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": { "contents": { "kind": "plaintext", "value": text } }
                            });
                            if framed
                                .send(WireMessage::LspPayload(resp.to_string()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                    "textDocument/didClose" => {
                        if let Some(uri) = val
                            .pointer("/params/textDocument/uri")
                            .and_then(|u| u.as_str())
                        {
                            open_docs.remove(uri);
                        }
                    }
                    _ => {}
                }
            }
            WireMessage::Disconnect { .. } => break,
            _ => {}
        }
    }
}

async fn run_end_to_end(mode: WorkspaceMode) {
    let remote = spawn_mock_gateway().await;
    let workdir = tempfile::tempdir().expect("failed to create test workdir");

    let workers = MIN_WORKERS + 2;
    let queries_per_worker = 2;
    let config = DivergentBenchConfig {
        remote,
        base_repo: None,
        workdir: Some(workdir.path().to_path_buf()),
        workers,
        queries_per_worker,
        keep_workdir: false,
        mode,
        persistent: false,
        churn_percent: 0,
    };

    let report = divergent_bench::run(config)
        .await
        .expect("divergent benchmark should complete against the mock gateway");

    assert_eq!(report.total_queries, workers * queries_per_worker);
    assert_eq!(
        report.total_errors, 0,
        "no queries should error against the mock gateway: {:?}",
        report.verifications
    );
    assert!(report.qps > 0.0, "throughput should be positive");
    assert!(report.latency.p50_ms >= 0.0);
    assert!(report.latency.p99_ms >= report.latency.p50_ms);

    for verification in &report.verifications {
        assert!(
            verification.passed,
            "expected correctness verification to pass for {}: {}",
            verification.kind.label(),
            verification.message
        );
    }
    assert!(report.all_passed, "overall report should report PASS");
    let expected_syncs = match mode {
        WorkspaceMode::Shared => 1,
        WorkspaceMode::Isolated => 4,
    };
    assert_eq!(report.initial_syncs.len(), expected_syncs);
    assert!(
        report
            .initial_syncs
            .iter()
            .all(|s| s.files > 0 && s.bytes > 0)
    );
}

#[tokio::test]
async fn divergent_bench_end_to_end_zero_bleed_shared() {
    run_end_to_end(WorkspaceMode::Shared).await;
}

#[tokio::test]
async fn divergent_bench_end_to_end_zero_bleed_isolated() {
    run_end_to_end(WorkspaceMode::Isolated).await;
}

#[tokio::test]
async fn divergent_bench_rejects_fewer_than_min_workers() {
    let remote = spawn_mock_gateway().await;
    let config = DivergentBenchConfig {
        remote,
        workers: MIN_WORKERS - 1,
        ..DivergentBenchConfig::default()
    };

    let err = divergent_bench::run(config)
        .await
        .expect_err("fewer than MIN_WORKERS workers must be rejected");
    assert!(err.to_string().contains("at least"));
}
