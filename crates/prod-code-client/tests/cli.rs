//! End-to-end integration tests for the `prod-code` CLI binary.
//!
//! Spawns `env!("CARGO_BIN_EXE_prod-code")` against an in-process mock gateway speaking the
//! wire protocol and asserts on stdout, stderr, exit codes, and output formats across all
//! subcommands and error conditions.

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ClusterResponse, ExecChunk, ExecExit, ExecMetric, HandshakeResponse, MetricsResponse,
    PROTOCOL_VERSION, PeerInfo, ProdCodeCodec, QueryMetric, ReadFileResponse, SearchHit,
    SearchResponse, ShadowHypothesisResult, ShadowRunResponse, StatusResponse, SyncProbeResponse,
    SyncResponse, WireMessage,
};
use prod_code_testkit::{Answer, ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

/// In-process mock gateway that supports all WireMessage variants and dispatches LSP methods.
pub struct MockGateway {
    pub addr: SocketAddr,
    pub calls: Arc<AtomicUsize>,
    pub fail_exec: Arc<AtomicBool>,
}

impl MockGateway {
    pub async fn start<F>(answer_lsp: F) -> Self
    where
        F: Fn(&str, &serde_json::Value) -> serde_json::Value + Send + Sync + 'static,
    {
        let lsp_fn: Answer = Arc::new(answer_lsp);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
        let addr = listener.local_addr().expect("local addr");
        let calls = Arc::new(AtomicUsize::new(0));
        let fail_exec = Arc::new(AtomicBool::new(false));

        let calls_clone = Arc::clone(&calls);
        let fail_exec_clone = Arc::clone(&fail_exec);

        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let lsp = Arc::clone(&lsp_fn);
                let calls = Arc::clone(&calls_clone);
                let fail_exec = Arc::clone(&fail_exec_clone);

                tokio::spawn(async move {
                    let _ = handle_client(socket, addr, lsp, calls, fail_exec).await;
                });
            }
        });

        Self {
            addr,
            calls,
            fail_exec,
        }
    }
}

async fn handle_client(
    socket: TcpStream,
    local_addr: SocketAddr,
    lsp_fn: Answer,
    calls: Arc<AtomicUsize>,
    fail_exec: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    while let Some(msg_res) = framed.next().await {
        let msg = match msg_res {
            Ok(m) => m,
            Err(_) => break,
        };
        match msg {
            WireMessage::SyncProbeRequest(req) => {
                framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::SyncRequest(req) => {
                framed
                    .send(WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: req.client_workspace_root,
                        files_updated: 0,
                        files_deleted: 0,
                        bytes_transferred: 0,
                        duration_ms: 1,
                        workspace_was_fresh: false,
                        stale_paths: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::HandshakeRequest(req) => {
                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version: PROTOCOL_VERSION,
                        server_pid: std::process::id(),
                        session_id: 1,
                        server_workspace_root: req.client_workspace_root,
                        detected_engine: "rust".to_string(),
                        stale_paths: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::ClusterRequest => {
                framed
                    .send(WireMessage::ClusterResponse(ClusterResponse {
                        this_node: local_addr.to_string(),
                        nodes: vec![PeerInfo {
                            addr: local_addr.to_string(),
                            status: StatusResponse {
                                server_pid: std::process::id(),
                                uptime_seconds: 3600,
                                active_sessions: 1,
                                loaded_workspaces: 1,
                                detected_engines: vec!["rust (rust-analyzer)".to_string()],
                                memory_rss_bytes: Some(1024 * 1024 * 128),
                                total_queries: 10,
                                active_queries: 0,
                                load_average_millis: Some(100),
                                cpu_count: Some(8),
                                platform: None,
                                running_commands: Vec::new(),
                            },
                            last_seen_secs: 0,
                            workspaces: vec![],
                            alive: true,
                        }],
                    }))
                    .await?;
            }
            WireMessage::StatusRequest => {
                framed
                    .send(WireMessage::StatusResponse(StatusResponse {
                        server_pid: std::process::id(),
                        uptime_seconds: 7265,
                        active_sessions: 2,
                        loaded_workspaces: 1,
                        detected_engines: vec!["rust".to_string(), "go".to_string()],
                        memory_rss_bytes: Some(1024 * 1024 * 64),
                        total_queries: 42,
                        active_queries: 0,
                        load_average_millis: Some(200),
                        cpu_count: Some(4),
                        platform: None,
                        running_commands: vec![prod_code_protocol::RunningCommand {
                            workspace: "test-ws--wt-1a2b".to_string(),
                            command: "cargo test --workspace".to_string(),
                            running_seconds: 125,
                        }],
                    }))
                    .await?;
            }
            WireMessage::MetricsRequest(req) => {
                framed
                    .send(WireMessage::MetricsResponse(MetricsResponse {
                        node: local_addr.to_string(),
                        since_secs: req.since_secs,
                        events_in_memory: 5,
                        queries: vec![QueryMetric {
                            agent: "cli".to_string(),
                            host: "localhost".to_string(),
                            workspace: "test-ws".to_string(),
                            method: "textDocument/hover".to_string(),
                            count: 10,
                            errors: 0,
                            p50_ms: 1,
                            p95_ms: 3,
                            max_ms: 5,
                        }],
                        execs: vec![ExecMetric {
                            agent: "cli".to_string(),
                            host: "localhost".to_string(),
                            workspace: "test-ws".to_string(),
                            command: "cargo test".to_string(),
                            count: 2,
                            failures: 0,
                            total_ms: 300,
                        }],
                        sync_rounds: 1,
                        sync_files: 3,
                        sync_bytes: 2048,
                    }))
                    .await?;
            }
            WireMessage::ReadFileRequest(req) => {
                framed
                    .send(WireMessage::ReadFileResponse(ReadFileResponse {
                        path: req.path,
                        content: Some(b"pub fn mocked_remote_source() {}\n".to_vec()),
                        truncated: false,
                        error: None,
                    }))
                    .await?;
            }
            WireMessage::ExecRequest(req) => {
                let fail = fail_exec.load(Ordering::Relaxed);
                if fail {
                    framed
                        .send(WireMessage::ExecChunk(ExecChunk {
                            stderr: true,
                            data: Some(b"error: failed to compile\n".to_vec()),
                        }))
                        .await?;
                    framed
                        .send(WireMessage::ExecExit(ExecExit {
                            exit_code: Some(1),
                            duration_ms: 10,
                            server_workspace_root: req.client_workspace_root,
                            timed_out: false,
                            error: None,
                            usage: None,
                            platform: None,
                        }))
                        .await?;
                } else {
                    // A variable the test set comes back as a passing test named after it.
                    let mut stdout = String::new();
                    for (key, value) in req.env.iter().filter(|(k, _)| k.starts_with("ECHO_")) {
                        stdout.push_str(&format!("test {key}={value} ... ok\n"));
                    }
                    stdout.push_str("test result: ok. 1 passed; 0 failed\n");
                    framed
                        .send(WireMessage::ExecChunk(ExecChunk {
                            stderr: false,
                            data: Some(stdout.into_bytes()),
                        }))
                        .await?;
                    framed
                        .send(WireMessage::ExecExit(ExecExit {
                            exit_code: Some(0),
                            duration_ms: 10,
                            server_workspace_root: req.client_workspace_root,
                            timed_out: false,
                            error: None,
                            usage: Some(prod_code_protocol::ExecUsage {
                                cpu_user_ms: 1500,
                                cpu_sys_ms: 200,
                                max_rss_kb: 10240,
                            }),
                            platform: Some("linux x86_64".to_string()),
                        }))
                        .await?;
                }
            }
            WireMessage::SearchRequest(req) => {
                framed
                    .send(WireMessage::SearchResponse(SearchResponse {
                        server_workspace_root: req.client_workspace_root,
                        hits: vec![SearchHit {
                            file: "src/lib.rs".to_string(),
                            line: 1,
                            kind: "function".to_string(),
                            name: "foo".to_string(),
                            container: None,
                            signature: "pub fn foo()".to_string(),
                            doc: "Doc comment for foo".to_string(),
                        }],
                        indexed_files: 1,
                        indexed_declarations: 1,
                        took_ms: 5,
                        error: None,
                        dense: None,
                    }))
                    .await?;
            }
            WireMessage::ShadowRunRequest(req) => {
                framed
                    .send(WireMessage::ShadowRunResponse(ShadowRunResponse {
                        server_workspace_root: req.client_workspace_root,
                        mode: "overlay".to_string(),
                        results: vec![ShadowHypothesisResult {
                            name: "hypo1".to_string(),
                            exit_code: Some(0),
                            duration_ms: 15,
                            timed_out: false,
                            error: None,
                            output_tail: Some(b"all 5 tests passed\n".to_vec()),
                            output_len: 19,
                        }],
                        error: None,
                    }))
                    .await?;
            }
            WireMessage::LspPayload(json) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
                    continue;
                };
                let Some(id) = value.get("id").cloned() else {
                    continue; // notification: initialized, didOpen, etc.
                };
                let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let params = value
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let result = if method == "initialize" {
                    serde_json::json!({
                        "capabilities": {
                            "hoverProvider": true,
                            "definitionProvider": true,
                            "documentSymbolProvider": true,
                            "referencesProvider": true
                        }
                    })
                } else {
                    calls.fetch_add(1, Ordering::Relaxed);
                    lsp_fn(method, &params)
                };
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result
                });
                framed
                    .send(WireMessage::LspPayload(response.to_string()))
                    .await?;
            }
            WireMessage::Disconnect { .. } => break,
            _ => {}
        }
    }
    Ok(())
}

fn make_workspace() -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub struct Order {\n    pub order_id: String,\n}\n\npub fn calculate() -> i32 {\n    42\n}\n",
        ),
    ])
}

async fn run_cli(ws: &Workspace, remote: SocketAddr, args: &[&str]) -> Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(remote.to_string())
        .args(args)
        .env("PROD_CODE_REMOTE", remote.to_string())
        .env("HOME", ws.root())
        .current_dir(ws.root())
        .output()
        .await
        .expect("run prod-code")
}

async fn run_cli_with_stdin(
    ws: &Workspace,
    remote: SocketAddr,
    args: &[&str],
    input: &[u8],
) -> Output {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(remote.to_string())
        .args(args)
        .env("PROD_CODE_REMOTE", remote.to_string())
        .env("HOME", ws.root())
        .current_dir(ws.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn prod-code");

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(input).await;
        drop(stdin);
    }
    child.wait_with_output().await.expect("wait prod-code")
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[tokio::test]
async fn cli_reports_status_and_health_from_gateway() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;
    let out = run_cli(&ws, gw.addr, &["status"]).await;
    assert!(out.status.success());
    let stdout = stdout_of(&out);
    assert!(stdout.contains("prod-code Remote Code Intelligence Gateway"));
    assert!(stdout.contains("Status:            HEALTHY"));
    assert!(stdout.contains("Server PID:"));
    // A build running on the node is shown, so the node is not taken for idle (#273).
    assert!(stdout.contains("Running Commands:  1"), "{stdout}");
    assert!(
        stdout.contains("  • test-ws--wt-1a2b  2m 5s  cargo test --workspace"),
        "{stdout}"
    );
}

#[tokio::test]
async fn cli_reports_cluster_membership_and_placement() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;
    let out = run_cli(&ws, gw.addr, &["cluster"]).await;
    assert!(out.status.success());
    let stdout = stdout_of(&out);
    assert!(stdout.contains("prod-code cluster"));
    assert!(stdout.contains("UP"));
}

#[tokio::test]
async fn cli_reports_metrics_in_human_readable_and_json_formats() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out_text = run_cli(&ws, gw.addr, &["metrics"]).await;
    assert!(out_text.status.success());
    assert!(stdout_of(&out_text).contains("prod-code usage"));

    let out_json = run_cli(&ws, gw.addr, &["metrics", "--json"]).await;
    assert!(out_json.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout_of(&out_json)).expect("metrics json parse");
    assert!(parsed.is_array());
}

#[tokio::test]
async fn cli_syncs_workspace_delta_to_gateway() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(&ws, gw.addr, &["sync"]).await;
    assert!(out.status.success());
    let stdout = stdout_of(&out);
    assert!(stdout.contains("prod-code Fast-Sync Completed"));
    assert!(stdout.contains("Status:            SYNCHRONIZED"));

    let out_sub = run_cli(&ws, gw.addr, &["sync", "src/lib.rs"]).await;
    assert!(out_sub.status.success());
    assert!(stdout_of(&out_sub).contains("SYNCHRONIZED"));
}

#[tokio::test]
async fn cli_resolves_definition_location_and_reports_when_none_found() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p1 = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/definition" => answers::locations(&p1, &[(1, 5)]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["def", "src/lib.rs", "1", "5"]).await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("📍 Definition:"));

    let gw_empty = MockGateway::start(|method, _| match method {
        "textDocument/definition" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out_empty = run_cli(&ws, gw_empty.addr, &["def", "src/lib.rs", "1", "5"]).await;
    assert!(out_empty.status.success());
    assert!(stdout_of(&out_empty).contains("No definition found."));
}

#[tokio::test]
async fn cli_renders_hover_markdown_contents() {
    let ws = make_workspace();
    let gw = MockGateway::start(|method, _| match method {
        "textDocument/hover" => answers::hover("### Order\nRepresents a trade order."),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["hover", "src/lib.rs", "1", "12"]).await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("Represents a trade order."));
}

#[tokio::test]
async fn scripted_gateway_answers_hover_query() {
    let ws = make_workspace();
    let gateway = ScriptedGateway::start(|method, _params| match method {
        "textDocument/hover" => answers::hover("hover via scripted gateway"),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gateway.addr(), &["hover", "src/lib.rs", "1", "5"]).await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("hover via scripted gateway"));
}

#[tokio::test]
async fn cli_finds_references_and_reports_when_none_found() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/references" => answers::locations(&p, &[(1, 12), (3, 5)]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["refs", "src/lib.rs", "1", "12"]).await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("Found 2 reference(s):"));

    let gw_empty = MockGateway::start(|method, _| match method {
        "textDocument/references" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out_empty = run_cli(&ws, gw_empty.addr, &["refs", "src/lib.rs", "1", "12"]).await;
    assert!(out_empty.status.success());
    assert!(stdout_of(&out_empty).contains("No references found."));
}

#[tokio::test]
async fn cli_lists_incoming_callers_and_handles_none_found() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => serde_json::json!([{
            "name": "calculate",
            "kind": 12,
            "uri": format!("file://{}", p.display()),
            "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 6, "character": 1 } },
            "selectionRange": { "start": { "line": 4, "character": 7 }, "end": { "line": 4, "character": 16 } }
        }]),
        "callHierarchy/incomingCalls" => serde_json::json!([{
            "from": {
                "name": "main",
                "kind": 12,
                "uri": format!("file://{}", p.display()),
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 2, "character": 1 } },
                "selectionRange": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 11 } }
            },
            "fromRanges": [{ "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 13 } }]
        }]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["callers", "src/lib.rs", "5", "8"]).await;
    assert!(out.status.success());
    let stdout = stdout_of(&out);
    assert!(stdout.contains("`calculate`: 1 caller(s)"));
    assert!(stdout.contains("main"));

    let gw_empty = MockGateway::start(|method, _| match method {
        "textDocument/prepareCallHierarchy" => serde_json::json!([{
            "name": "calculate",
            "kind": 12,
            "uri": "file:///src/lib.rs",
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 1 } },
            "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }
        }]),
        "callHierarchy/incomingCalls" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out_empty = run_cli(&ws, gw_empty.addr, &["callers", "src/lib.rs", "5", "8"]).await;
    assert!(out_empty.status.success());
    assert!(stdout_of(&out_empty).contains("no callers found."));
}

#[tokio::test]
async fn cli_lists_outgoing_callees_and_handles_none_found() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => serde_json::json!([{
            "name": "calculate",
            "kind": 12,
            "uri": format!("file://{}", p.display()),
            "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 6, "character": 1 } },
            "selectionRange": { "start": { "line": 4, "character": 7 }, "end": { "line": 4, "character": 16 } }
        }]),
        "callHierarchy/outgoingCalls" => serde_json::json!([{
            "to": {
                "name": "helper",
                "kind": 12,
                "uri": format!("file://{}", p.display()),
                "range": { "start": { "line": 8, "character": 0 }, "end": { "line": 10, "character": 1 } },
                "selectionRange": { "start": { "line": 8, "character": 3 }, "end": { "line": 8, "character": 9 } }
            },
            "fromRanges": [{ "start": { "line": 5, "character": 4 }, "end": { "line": 5, "character": 10 } }]
        }]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["callees", "src/lib.rs", "5", "8"]).await;
    assert!(out.status.success());
    let stdout = stdout_of(&out);
    assert!(stdout.contains("`calculate`: 1 callee(s)"));
    assert!(stdout.contains("helper"));

    let gw_empty = MockGateway::start(|method, _| match method {
        "textDocument/prepareCallHierarchy" => serde_json::json!([{
            "name": "calculate",
            "kind": 12,
            "uri": "file:///src/lib.rs",
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 1 } },
            "selectionRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }
        }]),
        "callHierarchy/outgoingCalls" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out_empty = run_cli(&ws, gw_empty.addr, &["callees", "src/lib.rs", "5", "8"]).await;
    assert!(out_empty.status.success());
    assert!(stdout_of(&out_empty).contains("no callees found."));
}

#[tokio::test]
async fn cli_lists_implementations_and_handles_none_found() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/implementation" => answers::locations(&p, &[(1, 1)]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["impls", "src/lib.rs", "1", "1"]).await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("Found 1 implementation(s):"));

    let gw_empty = MockGateway::start(|method, _| match method {
        "textDocument/implementation" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out_empty = run_cli(&ws, gw_empty.addr, &["impls", "src/lib.rs", "1", "1"]).await;
    assert!(out_empty.status.success());
    assert!(stdout_of(&out_empty).contains("No implementations found."));
}

#[tokio::test]
async fn cli_lists_document_symbols_with_kinds_and_line_numbers() {
    let ws = make_workspace();
    let gw = MockGateway::start(|method, _| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("Order", 23, 1, 3, 5),
            answers::document_symbol("calculate", 12, 5, 7, 8)
        ]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["symbols", "src/lib.rs"]).await;
    assert!(out.status.success());
    let stdout = stdout_of(&out);
    assert!(stdout.contains("[Struct] Order (line 1)"));
    assert!(stdout.contains("[Function] calculate (line 5)"));
}

#[tokio::test]
async fn cli_checks_diagnostics_clean_and_with_errors_exiting_nonzero() {
    let ws = make_workspace();
    let gw_clean = MockGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;

    let out_clean = run_cli(&ws, gw_clean.addr, &["diagnostics", "src/lib.rs"]).await;
    assert!(out_clean.status.success());
    assert!(stdout_of(&out_clean).contains("0 error(s)"));

    let gw_err = MockGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::error_at(1, 1, "E0001", "syntax error detected"),
        _ => serde_json::Value::Null,
    })
    .await;

    let out_err = run_cli(&ws, gw_err.addr, &["diagnostics", "src/lib.rs"]).await;
    assert_eq!(out_err.status.code(), Some(1));
    assert!(stdout_of(&out_err).contains("1 error(s)"));
    assert!(stdout_of(&out_err).contains("syntax error detected"));

    let out_json = run_cli(&ws, gw_clean.addr, &["diagnostics", "src/lib.rs", "--json"]).await;
    assert!(out_json.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout_of(&out_json)).expect("parse diagnostics json");
    assert_eq!(parsed["errors"], 0);
}

#[tokio::test]
async fn cli_validates_proposed_content_from_file_and_stdin() {
    let ws = make_workspace();
    let proposed_path = ws.write("src/proposed.rs", "pub fn proposed() -> i32 { 10 }\n");

    let gw = MockGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;

    let out_file = run_cli(
        &ws,
        gw.addr,
        &[
            "validate",
            "src/lib.rs",
            "--from",
            proposed_path.to_str().unwrap(),
        ],
    )
    .await;
    assert!(out_file.status.success());
    assert!(stdout_of(&out_file).contains("0 error(s)"));

    let out_stdin = run_cli_with_stdin(
        &ws,
        gw.addr,
        &["validate", "src/lib.rs"],
        b"pub fn from_stdin() {}\n",
    )
    .await;
    assert!(out_stdin.status.success());
    assert!(stdout_of(&out_stdin).contains("0 error(s)"));

    let gw_err = MockGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::error_at(1, 1, "E0999", "invalid replacement text"),
        _ => serde_json::Value::Null,
    })
    .await;

    let out_stdin_err = run_cli_with_stdin(
        &ws,
        gw_err.addr,
        &["validate", "src/lib.rs"],
        b"broken code\n",
    )
    .await;
    assert_eq!(out_stdin_err.status.code(), Some(1));
}

#[tokio::test]
async fn cli_scans_for_dead_code_reporting_unreferenced_items() {
    let ws = make_workspace();
    let gw = MockGateway::start(|method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("unused_item", 12, 1, 2, 8)])
        }
        "textDocument/references" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["dead-code"]).await;
    assert!(out.status.success());
    let stdout = stdout_of(&out);
    assert!(stdout.contains("dead code scan"));

    let out_json = run_cli(&ws, gw.addr, &["dead-code", "--json"]).await;
    assert!(out_json.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout_of(&out_json)).expect("dead-code json parse");
    assert!(parsed.get("dead").is_some());
}

#[tokio::test]
async fn cli_reads_remote_source_file_with_line_context() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "source",
            "/usr/lib/rust/lib.rs",
            "--line",
            "1",
            "--context",
            "5",
        ],
    )
    .await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("mocked_remote_source"));
}

#[tokio::test]
async fn cli_lists_assists_and_applies_assist_with_edits() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "prodCode/assists" => serde_json::json!([{
            "id": "inline_fn",
            "kind": "refactor.inline",
            "label": "Inline function",
            "subtype": 1
        }]),
        "prodCode/applyAssist" => answers::whole_file(
            &p,
            "pub struct Order {\n    pub order_id: String,\n}\n\npub fn calculate() -> i32 {\n    42\n}\n",
            "pub struct Order {\n    pub order_id: String,\n}\n\npub fn calculate() -> i32 {\n    100\n}\n",
        ),
        _ => serde_json::Value::Null,
    })
    .await;

    let out_list = run_cli(&ws, gw.addr, &["assists", "src/lib.rs", "5", "8"]).await;
    assert!(out_list.status.success());
    assert!(
        stdout_of(&out_list).contains("inline_fn --subtype 1  [refactor.inline]  Inline function")
    );

    let out_apply = run_cli(
        &ws,
        gw.addr,
        &[
            "assist",
            "src/lib.rs",
            "5",
            "8",
            "inline_fn",
            "--subtype",
            "1",
        ],
    )
    .await;
    assert!(out_apply.status.success());
    assert!(stdout_of(&out_apply).contains("applied `inline_fn`"));
}

#[tokio::test]
async fn cli_performs_safe_delete_updating_local_file() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "prodCode/safeDelete" => answers::whole_file(
            &p,
            "pub struct Order {\n    pub order_id: String,\n}\n\npub fn calculate() -> i32 {\n    42\n}\n",
            "pub struct Order {\n    pub order_id: String,\n}\n",
        ),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["safe-delete", "src/lib.rs", "5", "8"]).await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(
        stdout_of(&out).contains("deleted; 1 path(s) updated"),
        "{}",
        stdout_of(&out)
    );
    assert!(!ws.read("src/lib.rs").contains("calculate"));
}

#[tokio::test]
async fn cli_renames_symbol_across_workspace_and_applies_edits() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/rename" => answers::whole_file(
            &p,
            "pub struct Order {\n    pub order_id: String,\n}\n\npub fn calculate() -> i32 {\n    42\n}\n",
            "pub struct Trade {\n    pub order_id: String,\n}\n\npub fn calculate() -> i32 {\n    42\n}\n",
        ),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["rename", "src/lib.rs", "1", "12", "Trade"]).await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("renamed to `Trade`"));
    assert!(ws.read("src/lib.rs").contains("pub struct Trade"));
}

#[tokio::test]
async fn cli_runs_remote_check_lint_and_test_with_exit_codes() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out_check = run_cli(&ws, gw.addr, &["check"]).await;
    assert!(out_check.status.success());

    let out_check_json = run_cli(&ws, gw.addr, &["check", "--json"]).await;
    assert!(out_check_json.status.success());

    let out_lint = run_cli(&ws, gw.addr, &["lint"]).await;
    assert!(out_lint.status.success());

    let out_test = run_cli(&ws, gw.addr, &["test", "calc_filter"]).await;
    assert!(out_test.status.success());

    gw.fail_exec.store(true, Ordering::Relaxed);

    let out_check_fail = run_cli(&ws, gw.addr, &["check"]).await;
    assert_eq!(out_check_fail.status.code(), Some(1));

    let out_lint_fail = run_cli(&ws, gw.addr, &["lint"]).await;
    assert_eq!(out_lint_fail.status.code(), Some(1));

    let out_test_fail = run_cli(&ws, gw.addr, &["test"]).await;
    assert_eq!(out_test_fail.status.code(), Some(1));
}

#[tokio::test]
async fn cli_test_sets_env_and_prints_events_then_the_report() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "test",
            "--env",
            "ECHO_A=1",
            "--env",
            "ECHO_B=x=y",
            "--events",
        ],
    )
    .await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let lines: Vec<serde_json::Value> = stdout_of(&out)
        .lines()
        .map(|l| serde_json::from_str(l).expect("every line is JSON"))
        .collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(
        lines[0],
        serde_json::json!({ "event": "test", "name": "ECHO_A=1", "ok": true })
    );
    assert_eq!(lines[1]["name"], "ECHO_B=x=y");
    assert_eq!(lines[2]["event"], "report");
    assert_eq!(lines[2]["report"]["usage"]["cpu_user_ms"], 1500);

    let text = run_cli(&ws, gw.addr, &["test"]).await;
    assert!(
        stdout_of(&text).contains("cpu 1.5s user 0.2s sys, peak 10 MB"),
        "{}",
        stdout_of(&text)
    );
    assert!(
        stdout_of(&text).contains(" on linux x86_64;"),
        "the platform is named: {}",
        stdout_of(&text)
    );

    let bad = run_cli(&ws, gw.addr, &["check", "--env", "NOEQUALS"]).await;
    assert_eq!(bad.status.code(), Some(1));
    assert!(
        stderr_of(&bad).contains("--env takes KEY=VALUE"),
        "{}",
        stderr_of(&bad)
    );
    let with_fix = run_cli(&ws, gw.addr, &["lint", "--fix", "--env", "A=1"]).await;
    assert_eq!(with_fix.status.code(), Some(2));
}

#[tokio::test]
async fn cli_executes_remote_commands_and_streams_output() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(
        &ws,
        gw.addr,
        &["exec", "--no-pull", "--", "cargo", "test", "-p", "fixture"],
    )
    .await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("test result: ok"));

    gw.fail_exec.store(true, Ordering::Relaxed);
    let out_fail = run_cli(&ws, gw.addr, &["exec", "--", "cargo", "build"]).await;
    assert_eq!(out_fail.status.code(), Some(1));
    assert!(stderr_of(&out_fail).contains("error: failed to compile"));
}

#[tokio::test]
async fn cli_diagnoses_test_suite_failures() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(&ws, gw.addr, &["diagnose"]).await;
    assert!(out.status.success());

    let out_json = run_cli(&ws, gw.addr, &["diagnose", "--json"]).await;
    assert!(out_json.status.success());
}

#[tokio::test]
async fn cli_analyzes_git_diff_impact() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(&ws, gw.addr, &["impact"]).await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("impact of HEAD"));

    let out_json = run_cli(&ws, gw.addr, &["impact", "--json"]).await;
    assert!(out_json.status.success());
}

#[tokio::test]
async fn cli_generates_fixture_for_named_type() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([answers::symbol("Order", 23, &p, 1, 1)]),
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("Order", 23, 1, 3, 5)])
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["fixture", "Order", "--no-verify"]).await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("fixture for `Order`"));
}

#[tokio::test]
async fn cli_renames_schema_field_across_languages() {
    let ws = make_workspace();
    let gw = MockGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(
        &ws,
        gw.addr,
        &["schema-rename", "order_id", "--to", "trade_id"],
    )
    .await;
    assert!(out.status.success());
}

#[tokio::test]
async fn cli_changes_function_signature() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([answers::symbol("calculate", 12, &p, 5, 8)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(
        &ws,
        gw.addr,
        &["change-signature", "calculate", "--param", "x: i32 = 0"],
    )
    .await;
    assert!(out.status.success());
}

#[tokio::test]
async fn cli_applies_structural_codemod_rule() {
    let ws = make_workspace();
    let gw = MockGateway::start(|method, _| match method {
        "prodCode/structuralReplace" => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "codemod",
            "$a.unwrap() ==>> $a.expect(\"invariant\")",
            "--path",
            "src/lib.rs",
        ],
    )
    .await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("matches nothing"));
}

#[tokio::test]
async fn cli_searches_declarations_by_intent() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(&ws, gw.addr, &["search", "calculate order total"]).await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("foo"));
}

#[tokio::test]
async fn cli_slices_symbol_dependencies() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([answers::symbol("calculate", 12, &p, 5, 8)]),
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("calculate", 12, 5, 7, 8)])
        }
        "callHierarchy/outgoingCalls" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["slice", "calculate"]).await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("calculate"));

    let out_line = run_cli(&ws, gw.addr, &["slice", "src/lib.rs", "--line", "5"]).await;
    assert!(out_line.status.success());
}

#[tokio::test]
async fn cli_runs_shadow_hypotheses() {
    let ws = make_workspace();
    let spec_path = ws.write(
        "spec.json",
        r#"{"hypotheses":[{"name":"h1","edits":[{"path":"src/lib.rs","new_text":"pub fn b() {}\n"}]}]}"#,
    );
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "shadow-run",
            spec_path.to_str().unwrap(),
            "--",
            "cargo",
            "test",
        ],
    )
    .await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("hypo1"));
}

#[tokio::test]
async fn cli_runs_pipelined_benchmark() {
    let ws = make_workspace();
    let gw = MockGateway::start(|method, _| match method {
        "textDocument/hover" => answers::hover("benchmark hover content"),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "bench",
            "--concurrency",
            "1",
            "--depth",
            "1",
            "--duration-secs",
            "1",
        ],
    )
    .await;
    assert!(out.status.success());
    assert!(stdout_of(&out).contains("Benchmark Results:"));
}

#[tokio::test]
async fn cli_serves_mcp_protocol_over_stdio() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("mcp")
        .arg("--remote")
        .arg(gw.addr.to_string())
        .env("HOME", ws.root())
        .current_dir(ws.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcp");

    let init_line = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"0.1\"}}}\n";
    let list_line = "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n";

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(init_line.as_bytes()).await;
        let _ = stdin.write_all(list_line.as_bytes()).await;
        drop(stdin);
    }

    let output = child.wait_with_output().await.expect("wait mcp");
    assert!(output.status.success());
    let stdout = stdout_of(&output);
    assert!(stdout.contains("\"tools\""));
}

#[tokio::test]
async fn cli_bridges_lsp_protocol_over_stdio() {
    let ws = make_workspace();
    let gw = MockGateway::start(|method, _| match method {
        "initialize" => serde_json::json!({ "capabilities": {} }),
        _ => serde_json::Value::Null,
    })
    .await;

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("lsp")
        .arg("--remote")
        .arg(gw.addr.to_string())
        .env("HOME", ws.root())
        .current_dir(ws.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lsp");

    let init_payload =
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{}}}";
    let msg = format!(
        "Content-Length: {}\r\n\r\n{}",
        init_payload.len(),
        init_payload
    );

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(msg.as_bytes()).await;
        drop(stdin);
    }

    let output = child.wait_with_output().await.expect("wait lsp");
    assert!(output.status.success());
}

#[tokio::test]
async fn cli_rejects_missing_required_arguments_with_clap_error() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(&ws, gw.addr, &["def"]).await;
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr_of(&out).contains("required"));
}

#[tokio::test]
async fn cli_rejects_invalid_numeric_positions() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(&ws, gw.addr, &["def", "src/lib.rs", "not_a_line", "5"]).await;
    assert_eq!(out.status.code(), Some(2));
}

#[tokio::test]
async fn cli_rejects_unknown_subcommands() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(&ws, gw.addr, &["bogus_subcommand"]).await;
    assert_eq!(out.status.code(), Some(2));
}

#[tokio::test]
async fn cli_rejects_invalid_assist_to_format() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(
        &ws,
        gw.addr,
        &["assists", "src/lib.rs", "1", "1", "--to", "invalid_range"],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_of(&out).contains("expected LINE:COL"));
}

#[tokio::test]
async fn cli_rejects_divergent_bench_with_too_few_workers() {
    let ws = make_workspace();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;

    let out = run_cli(&ws, gw.addr, &["divergent-bench", "--workers", "3"]).await;
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr_of(&out).contains("at least 10 concurrent workers"));
}

/// The three refactorings that write whole files: each one reports a diff and, without
/// `--apply`, leaves the checkout alone.
#[tokio::test]
async fn cli_extracts_a_parameter_and_gives_every_caller_the_argument() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub fn render(text: &str) -> String {\n    let width = 80;\n    format!(\"{text}{width}\")\n}\n\npub fn caller() -> String {\n    render(\"x\")\n}\n",
        ),
    ]);
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("render", 12, 1, 4, 8),
            answers::document_symbol("caller", 12, 6, 8, 8),
        ]),
        "textDocument/references" => answers::locations(&p, &[(7, 5)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "extract-parameter",
            "src/lib.rs",
            "2",
            "17",
            "--to",
            "2:19",
            "--name",
            "width_limit",
            "--type",
            "usize",
        ],
    )
    .await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.contains("width_limit: usize"), "{text}");
    assert!(text.contains("render(\"x\", 80)"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
    assert!(
        ws.read("src/lib.rs").contains("let width = 80;"),
        "the checkout is untouched without --apply"
    );

    // A malformed selection is rejected before anything is asked of the gateway.
    let bad = run_cli(
        &ws,
        gw.addr,
        &[
            "extract-parameter",
            "src/lib.rs",
            "2",
            "17",
            "--to",
            "nonsense",
            "--name",
            "x",
        ],
    )
    .await;
    assert!(!bad.status.success());
    assert!(stderr_of(&bad).contains("LINE:COL"), "{}", stderr_of(&bad));
}

#[tokio::test]
async fn cli_bundles_parameters_into_a_struct() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub fn build(name: &str, width: u32, height: u32) -> String {\n    let area = width * height;\n    format!(\"{name}{area}\")\n}\n\npub fn caller() -> String {\n    build(\"a\", 3, 4)\n}\n",
        ),
    ]);
    let path = ws.path("src/lib.rs");
    let p = path.clone();

    let gw = MockGateway::start(move |method, params| match method {
        "workspace/symbol" => serde_json::json!([answers::symbol("build", 12, &p, 1, 8)]),
        "textDocument/references" => {
            let ch = params
                .pointer("/position/character")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            match ch {
                7 => answers::locations(&p, &[(7, 5)]),
                25 => answers::locations(&p, &[(2, 16)]),
                37 => answers::locations(&p, &[(2, 24)]),
                _ => serde_json::json!([]),
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "parameter-object",
            "build",
            "--param",
            "width",
            "--param",
            "height",
            "--name",
            "Size",
        ],
    )
    .await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.contains("pub struct Size"), "{text}");
    assert!(text.contains("Size { width: 3, height: 4 }"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
}

#[tokio::test]
async fn cli_moves_a_declaration_to_another_module() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub mod home;\npub mod other;\n"),
        (
            "src/home.rs",
            "/// Says the name.\npub fn describe(p: &str) -> String {\n    p.to_string()\n}\n",
        ),
        ("src/other.rs", "//! The new home.\n"),
    ]);
    let home = ws.path("src/home.rs");
    let h = home.clone();

    let gw = MockGateway::start(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([answers::symbol("describe", 12, &h, 2, 8)]),
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("describe", 12, 2, 4, 8)])
        }
        "textDocument/references" => serde_json::json!([]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["move", "describe", "--to", "src/other.rs"]).await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.contains("`describe` moved"), "{text}");
    assert!(
        text.contains("/// Says the name."),
        "the doc comment travels: {text}"
    );
    assert!(text.contains("nothing was written"), "{text}");
}

#[tokio::test]
async fn cli_migrates_a_declared_type_and_reports_what_no_longer_fits() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub struct Request {\n    pub timeout_secs: u64,\n}\n\npub fn use_it(r: &Request) -> u64 {\n    r.timeout_secs\n}\n",
        ),
    ]);

    // Each run pulls twice: the file on disk, which has no error, then the proposed text,
    // where the mismatch is the migration's.
    let pulls = std::sync::atomic::AtomicUsize::new(0);
    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/references" => serde_json::json!([]),
        "textDocument/diagnostic"
            if pulls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .is_multiple_of(2) =>
        {
            serde_json::json!({ "kind": "full", "items": [] })
        }
        "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [
            { "severity": 1, "code": "E0308", "message": "expected u64, found Duration",
              "range": { "start": { "line": 5, "character": 4 }, "end": { "line": 5, "character": 18 } } }
        ] }),
        _ => serde_json::Value::Null,
    })
    .await;

    // A field is rarely in the workspace symbol index, so this is the position form.
    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "migrate-type",
            "src/lib.rs",
            "--line",
            "2",
            "--character",
            "9",
            "--to",
            "std::time::Duration",
            "--convert",
        ],
    )
    .await;
    // Sites that do not fit are reported as an error exit, because they are work to do.
    assert!(!out.status.success());
    let text = stdout_of(&out);
    assert!(text.contains("was: `u64`"), "{text}");
    assert!(text.contains("now: `std::time::Duration`"), "{text}");
    assert!(text.contains("1 site(s) in 1 file(s)"), "{text}");
    assert!(text.contains("r.timeout_secs"), "{text}");
    assert!(text.contains("they are the migration"), "{text}");
    // `--convert` tried `.into()` there; the analyzer still rejects the line, so it was taken back.
    assert!(text.contains("was tried here"), "{text}");
    assert!(
        ws.read("src/lib.rs").contains("pub timeout_secs: u64,"),
        "nothing is written without --apply"
    );
}

#[tokio::test]
async fn cli_encapsulates_a_field_and_rewrites_the_accesses_outside_its_file() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub mod app;\npub mod config;\n"),
        (
            "src/config.rs",
            "pub struct Config {\n    pub name: String,\n}\n",
        ),
        (
            "src/app.rs",
            "use crate::config::Config;\n\npub fn shout(c: &Config) -> String {\n    c.name.to_uppercase()\n}\n",
        ),
    ]);
    let app = ws.path("src/app.rs");
    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/references" => answers::locations(&app, &[(4, 7)]),
        "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [] }),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "encapsulate-field",
            "src/config.rs",
            "--line",
            "2",
            "--character",
            "9",
        ],
    )
    .await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(
        text.contains("getter: `fn name(&self) -> &String`"),
        "{text}"
    );
    assert!(text.contains("+    c.name().to_uppercase()"), "{text}");
    assert!(
        text.contains("1 read(s) call a method on the field"),
        "a method call through a shared reference is flagged: {text}"
    );
    assert!(text.contains("nothing was written"), "{text}");
    assert!(ws.read("src/config.rs").contains("pub name: String"));
}

#[tokio::test]
async fn cli_extracts_a_field_and_initialises_it_where_the_struct_is_built() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub struct Store {\n    entries: Vec<u32>,\n}\n\nimpl Store {\n    pub fn new() -> Self {\n        Self { entries: Vec::new() }\n    }\n\n    pub fn limit(&self) -> usize {\n        let cap = 64 * 1024;\n        cap.min(self.entries.len())\n    }\n}\n",
        ),
    ]);
    let lib = ws.path("src/lib.rs");
    let def = lib.clone();
    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/definition" => answers::locations(&def, &[(1, 12)]),
        "textDocument/references" => answers::locations(&lib, &[(5, 6), (7, 9)]),
        "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [] }),
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "extract-field",
            "src/lib.rs",
            "11",
            "19",
            "--to",
            "11:28",
            "--name",
            "cap",
            "--type",
            "usize",
        ],
    )
    .await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.contains("new field: `cap: usize`"), "{text}");
    assert!(
        text.contains("+        Self { cap: 64 * 1024, entries: Vec::new() }"),
        "{text}"
    );
    assert!(text.contains("+        let cap = self.cap;"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
    assert!(ws.read("src/lib.rs").contains("let cap = 64 * 1024;"));
}

/// The CLI finds a declaration by name, gives a file's outline under its own name, and takes a
/// name instead of a position — what the MCP tools already did (#93).
#[tokio::test]
async fn cli_finds_by_name_outlines_a_file_and_takes_a_symbol_for_a_position() {
    let ws = make_workspace();
    let path = ws.path("src/lib.rs");
    let p = path.clone();
    let gw = MockGateway::start(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([answers::symbol("calculate", 12, &p, 5, 8)]),
        "textDocument/references" => answers::locations(&p, &[(5, 8)]),
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("calculate", 12, 5, 7, 8)])
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let found = run_cli(&ws, gw.addr, &["symbols", "calculate"]).await;
    assert!(found.status.success(), "{}", stderr_of(&found));
    let text = stdout_of(&found);
    assert!(
        text.contains("calculate") && text.contains("src/lib.rs:5:8"),
        "{text}"
    );

    let outline = run_cli(&ws, gw.addr, &["outline", "src/lib.rs"]).await;
    assert!(outline.status.success());
    assert!(stdout_of(&outline).contains("[Function] calculate (line 5)"));

    let refs = run_cli(&ws, gw.addr, &["refs", "--symbol", "calculate"]).await;
    assert!(refs.status.success(), "{}", stderr_of(&refs));
    assert!(
        stdout_of(&refs).contains("lib.rs:5"),
        "{}",
        stdout_of(&refs)
    );

    let neither = run_cli(&ws, gw.addr, &["refs"]).await;
    assert!(
        !neither.status.success(),
        "a position or a symbol is required"
    );
}

#[tokio::test]
async fn cli_outlines_a_directory_and_exits_zero() {
    let ws = make_workspace();
    ws.write("src/other.rs", "pub fn other() {}\n");
    ws.write("src/README.md", "# Module docs\n");
    let gw = MockGateway::start(|method, params| match method {
        "textDocument/documentSymbol" => {
            let uri = params
                .get("textDocument")
                .and_then(|t| t.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            if uri.ends_with("lib.rs") || uri.ends_with("other.rs") {
                serde_json::json!([answers::document_symbol("calculate", 12, 5, 7, 8)])
            } else {
                serde_json::Value::Null
            }
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let out = run_cli(&ws, gw.addr, &["outline", "src"]).await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let stdout = stdout_of(&out);
    assert!(stdout.contains("Outline for src/lib.rs:"), "{stdout}");
    assert!(stdout.contains("Outline for src/other.rs:"), "{stdout}");
    assert!(stdout.contains("[Function] calculate (line 5)"), "{stdout}");
    assert!(stdout.contains("2 file(s) outlined, 1 skipped"), "{stdout}");
}

#[tokio::test]
async fn cli_validates_several_files_together() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub mod other;\npub use other::VALUE;\n"),
        ("src/other.rs", "pub const VALUE: u32 = 1;\n"),
        (
            "proposed_lib.rs",
            "pub mod other;\npub use other::{VALUE, MORE};\n",
        ),
        (
            "proposed_other.rs",
            "pub const VALUE: u32 = 1;\npub const MORE: u32 = 2;\n",
        ),
    ]);
    let gw = MockGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "validate",
            "src/lib.rs",
            "--from",
            "proposed_lib.rs",
            "--with",
            "src/other.rs=proposed_other.rs",
        ],
    )
    .await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.contains("src/lib.rs: 0 error(s)"), "{text}");
    assert!(text.contains("src/other.rs: 0 error(s)"), "{text}");
    assert!(stderr_of(&out).contains("2 file(s) analysed together"));

    let bad = run_cli(
        &ws,
        gw.addr,
        &[
            "validate",
            "src/lib.rs",
            "--from",
            "proposed_lib.rs",
            "--with",
            "no-equals-sign",
        ],
    )
    .await;
    assert!(!bad.status.success());
    assert!(
        stderr_of(&bad).contains("--with takes FILE=NEW"),
        "{}",
        stderr_of(&bad)
    );
}

#[tokio::test]
async fn every_subcommand_has_its_own_help_line() {
    let ws = make_workspace();
    let out = run_cli(&ws, "127.0.0.1:1".parse().unwrap(), &["--help"]).await;
    let text = stdout_of(&out);
    let line_of = |name: &str| {
        text.lines()
            .find(|l| l.trim_start().starts_with(&format!("{name} ")))
            .unwrap_or_default()
            .to_string()
    };
    assert!(
        line_of("change-signature").contains("Change what a function takes"),
        "{text}"
    );
    assert!(
        !line_of("migrate-type").contains("what a function takes"),
        "{text}"
    );
    assert!(
        line_of("outline").contains("declarations of a file"),
        "{text}"
    );
}

#[tokio::test]
async fn cli_wraps_a_return_type_and_names_the_caller_that_cannot_propagate() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub fn plain() -> u32 {\n    5\n}\n\npub fn caller() -> u32 {\n    plain()\n}\n",
        ),
    ]);
    let lib = ws.path("src/lib.rs");
    let (l1, l2) = (lib.clone(), lib.clone());
    let gw = MockGateway::start(move |method, _| match method {
        "prodCode/applyAssist" => answers::whole_file(
            &l1,
            "pub fn plain() -> u32 {\n    5\n}\n\npub fn caller() -> u32 {\n    plain()\n}\n",
            "pub fn plain() -> Option<u32> {\n    Some(5)\n}\n\npub fn caller() -> u32 {\n    plain()\n}\n",
        ),
        "textDocument/references" => answers::locations(&l2, &[(6, 5)]),
        "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [] }),
        _ => serde_json::Value::Null,
    })
    .await;
    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "wrap-return",
            "src/lib.rs",
            "--line",
            "1",
            "--character",
            "8",
            "--wrapper",
            "option",
        ],
    )
    .await;
    assert!(!out.status.success(), "a blocked caller is an error exit");
    let text = stdout_of(&out);
    assert!(text.contains("now returns: `Option<u32>`"), "{text}");
    assert!(text.contains("the caller returns `u32`"), "{text}");
    assert!(
        ws.read("src/lib.rs").contains("-> u32 {\n    5"),
        "nothing written"
    );
}

#[tokio::test]
async fn cli_makes_a_method_static_and_rewrites_its_call() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub struct S;\n\nimpl S {\n    pub fn one(&self) -> u32 {\n        1\n    }\n}\n\npub fn f(s: &S) -> u32 {\n    s.one()\n}\n",
        ),
    ]);
    let lib = ws.path("src/lib.rs");
    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/references" => answers::locations(&lib, &[(10, 7)]),
        "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [] }),
        _ => serde_json::Value::Null,
    })
    .await;
    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "make-static",
            "src/lib.rs",
            "--line",
            "4",
            "--character",
            "12",
        ],
    )
    .await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.contains("+    pub fn one() -> u32 {"), "{text}");
    assert!(text.contains("+    S::one()"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
}

#[tokio::test]
async fn cli_converts_a_function_to_a_method_and_rewrites_its_call() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub struct S;\n\nimpl S {\n    pub fn one(s: &S) -> u32 {\n        let _ = s;\n        1\n    }\n}\n\npub fn f(s: &S) -> u32 {\n    S::one(s)\n}\n",
        ),
    ]);
    let lib = ws.path("src/lib.rs");
    let gw = MockGateway::start(move |method, params| {
        let character = params
            .pointer("/position/character")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        match method {
            // The parameter `s` (4:16), then the function `one` (4:12).
            "textDocument/references" if character == 15 => answers::locations(&lib, &[(5, 17)]),
            "textDocument/references" => answers::locations(&lib, &[(11, 8)]),
            "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [] }),
            _ => serde_json::Value::Null,
        }
    })
    .await;
    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "convert-to-method",
            "src/lib.rs",
            "--line",
            "4",
            "--character",
            "12",
        ],
    )
    .await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.contains("+    pub fn one(&self) -> u32 {"), "{text}");
    assert!(text.contains("+        let _ = self;"), "{text}");
    assert!(text.contains("+    s.one()"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
}

#[tokio::test]
async fn cli_inverts_a_predicate_and_its_call() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub fn is_on(x: u8) -> bool {\n    x > 0\n}\n\npub fn f(x: u8) -> bool {\n    is_on(x)\n}\n",
        ),
    ]);
    let lib = ws.path("src/lib.rs");
    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/references" => answers::locations(&lib, &[(6, 5)]),
        "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [] }),
        _ => serde_json::Value::Null,
    })
    .await;
    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "invert-boolean",
            "src/lib.rs",
            "--line",
            "1",
            "--character",
            "8",
            "--to",
            "is_off",
        ],
    )
    .await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.contains("+    !(x > 0)"), "{text}");
    assert!(text.contains("+    !is_off(x)"), "{text}");
}

#[tokio::test]
async fn cli_makes_a_parameter_generic() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "pub fn show(x: &u32) -> String {\n    x.to_string()\n}\n",
        ),
    ]);
    let gw = MockGateway::start(move |method, _| match method {
        "textDocument/references" => serde_json::json!([]),
        "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [] }),
        _ => serde_json::Value::Null,
    })
    .await;
    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "generify",
            "src/lib.rs",
            "--line",
            "1",
            "--character",
            "8",
            "--param",
            "x",
            "--bound",
            "std::fmt::Display",
            "--as",
            "D",
        ],
    )
    .await;
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(
        text.contains("now: `fn show<D: std::fmt::Display>(x: &D)`"),
        "{text}"
    );
    assert!(text.contains("nothing was written"), "{text}");
}

/// The refactoring subcommands added with roadmap 7.1.2, 7.1.5, 7.2 and 8.6 parse their
/// arguments and reach their tool: a clap error exits 2, anything the tool answers does not.
#[tokio::test]
async fn cli_new_refactoring_subcommands_parse_and_reach_their_tools() {
    let ws = make_workspace();
    let gw = MockGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    for args in [
        &[
            "extract-delegate",
            "src/lib.rs",
            "1",
            "5",
            "--fields",
            "order_id",
            "--name",
            "Id",
            "--field",
            "id",
        ][..],
        &[
            "extract-trait",
            "src/lib.rs",
            "1",
            "5",
            "--methods",
            "a,b",
            "--name",
            "T",
        ][..],
        &[
            "introduce-variable",
            "src/lib.rs",
            "6",
            "5",
            "--to",
            "6:7",
            "--name",
            "v",
        ][..],
        &["loop-to-iterator", "src/lib.rs", "5", "1"][..],
        &["prune", "--max-files", "5"][..],
        &["move-method", "src/lib.rs", "1", "8", "--to-param", "x"][..],
        &["validate", "--diff", "no-such.patch"][..],
        &["impact", "--ci", "--depth", "1"][..],
        &["move-method", "src/lib.rs", "1", "8", "--to-type", "X"][..],
        &["callers", "src/lib.rs", "5", "8", "--depth", "3"][..],
        &["supertypes", "src/lib.rs", "1", "12"][..],
        &["supertypes", "--symbol", "Cache"][..],
        &["callees", "--symbol", "calculate", "--depth", "2"][..],
        &[
            "schema-rename",
            "order_id",
            "--to",
            "trade_id",
            "--repo",
            ".",
        ][..],
        &[
            "move-module",
            "src/lib.rs",
            "--to",
            "src/x/lib.rs",
            "--verify",
            "compile",
        ][..],
        &[
            "extract-function",
            "src/lib.rs",
            "6",
            "5",
            "--to",
            "6:7",
            "--name",
            "f",
            "--no-duplicates",
            "--verify",
            "compile",
        ][..],
        &[
            "extract-function",
            "src/lib.rs",
            "6",
            "5",
            "--to",
            "6:7",
            "--name",
            "g",
            "--parameterize",
            "--other-files",
        ][..],
    ] {
        let out = run_cli(&ws, gw.addr, args).await;
        assert_ne!(out.status.code(), Some(2), "{args:?}: {}", stderr_of(&out));
        assert!(!stderr_of(&out).contains("unexpected argument"), "{args:?}");
    }
    // A malformed `--to` is the CLI's own error, not the tool's.
    let out = run_cli(
        &ws,
        gw.addr,
        &[
            "introduce-variable",
            "src/lib.rs",
            "6",
            "5",
            "--to",
            "six",
            "--name",
            "v",
        ],
    )
    .await;
    assert!(!out.status.success());
    assert!(
        stderr_of(&out).contains("--to takes LINE:COL"),
        "{}",
        stderr_of(&out)
    );
}

/// `prod-code lsp` in a project the node has never seen: the checkout is pushed before the
/// handshake, under the name the handshake then uses, and a file written while the editor runs
/// reaches the node on a connection of its own (#316). Before, the bridge pushed nothing, and
/// the node detected no language in an empty copy and answered every request with nothing.
#[tokio::test]
async fn lsp_pushes_the_checkout_before_its_handshake_and_every_change_after_it() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let ws = make_workspace();
    // The sync keeps its watermark under HOME: outside the checkout, or the watcher sees it.
    let home = tempfile::tempdir().expect("home");
    let seen: Arc<std::sync::Mutex<Vec<(usize, String)>>> = Arc::default();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        let mut connections = 0;
        while let Ok((socket, _)) = listener.accept().await {
            connections += 1;
            let (log, connection) = (Arc::clone(&log), connections);
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(msg)) = framed.next().await {
                    let (event, reply) = match msg {
                        WireMessage::SyncProbeRequest(req) => (
                            format!("probe {}", req.base_workspace_name.unwrap_or_default()),
                            Some(WireMessage::SyncProbeResponse(SyncProbeResponse {
                                server_workspace_root: req.client_workspace_root,
                                seeded: false,
                                files_deleted: 0,
                                missing: Vec::new(),
                            })),
                        ),
                        WireMessage::SyncRequest(req) => (
                            format!(
                                "sync {}",
                                req.files
                                    .iter()
                                    .map(|f| f.relative_path.as_str())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            ),
                            Some(WireMessage::SyncResponse(SyncResponse {
                                server_workspace_root: req.client_workspace_root,
                                files_updated: req.files.len(),
                                files_deleted: 0,
                                bytes_transferred: 0,
                                duration_ms: 1,
                                workspace_was_fresh: false,
                                stale_paths: Vec::new(),
                            })),
                        ),
                        WireMessage::HandshakeRequest(req) => (
                            format!(
                                "handshake {} purpose={}",
                                req.base_workspace_name.unwrap_or_default(),
                                req.purpose.unwrap_or_default()
                            ),
                            Some(WireMessage::HandshakeResponse(HandshakeResponse {
                                protocol_version: PROTOCOL_VERSION,
                                server_pid: std::process::id(),
                                session_id: 1,
                                server_workspace_root: req.client_workspace_root,
                                detected_engine: "rust".to_string(),
                                stale_paths: Vec::new(),
                            })),
                        ),
                        WireMessage::LspPayload(json) => {
                            let val: serde_json::Value =
                                serde_json::from_str(&json).unwrap_or_default();
                            let method = val["method"].as_str().unwrap_or_default().to_string();
                            let reply = (method == "initialize").then(|| {
                                WireMessage::LspPayload(
                                    serde_json::json!({ "jsonrpc": "2.0", "id": val["id"], "result": { "capabilities": {} } })
                                        .to_string(),
                                )
                            });
                            (format!("lsp {method}"), reply)
                        }
                        WireMessage::Disconnect { .. } => break,
                        _ => ("other".to_string(), None),
                    };
                    log.lock().expect("log").push((connection, event));
                    if let Some(reply) = reply
                        && framed.send(reply).await.is_err()
                    {
                        break;
                    }
                }
            });
        }
    });

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("lsp")
        .arg("--remote")
        .arg(addr.to_string())
        .env("HOME", home.path())
        .current_dir(ws.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lsp");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = tokio::io::BufReader::new(child.stdout.take().expect("stdout"));
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
    // Header names are case-insensitive, and some editors send them in lower case.
    stdin
        .write_all(format!("content-length: {}\r\n\r\n{init}", init.len()).as_bytes())
        .await
        .expect("write initialize");
    let mut header = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        stdout.read_line(&mut header),
    )
    .await
    .expect("initialize is answered")
    .expect("read the answer");
    assert!(header.starts_with("Content-Length:"), "{header}");

    ws.write("src/added.rs", "pub fn added() {}\n");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let pushed = |seen: &[(usize, String)]| {
        seen.iter().any(|(connection, event)| {
            *connection > 1 && event.starts_with("sync ") && event.contains("src/added.rs")
        })
    };
    while std::time::Instant::now() < deadline && !pushed(&seen.lock().expect("log")) {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    drop(stdin);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait()).await;

    let seen = seen.lock().expect("log").clone();
    let first: Vec<&String> = seen
        .iter()
        .filter(|(connection, _)| *connection == 1)
        .map(|(_, event)| event)
        .collect();
    let probe = first.iter().position(|e| e.starts_with("probe "));
    let handshake = first.iter().position(|e| e.starts_with("handshake "));
    assert!(
        matches!((probe, handshake), (Some(p), Some(h)) if p < h),
        "the checkout is pushed before the handshake: {seen:?}"
    );
    let name = |event: &str| event.split(' ').nth(1).map(str::to_string);
    assert_eq!(
        name(first[probe.unwrap()]),
        name(first[handshake.unwrap()]),
        "the handshake names the workspace the sync filled: {seen:?}"
    );
    assert!(
        first[handshake.unwrap()].ends_with("purpose=editor"),
        "the session says it is an editor's, so diagnostics are pushed to it: {seen:?}"
    );
    assert!(
        first.iter().any(|e| *e == "lsp initialize"),
        "the editor's messages go to the session: {seen:?}"
    );
    assert!(
        pushed(&seen),
        "a file written while the editor runs is pushed: {seen:?}"
    );
}
