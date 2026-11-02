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
use prod_code_mcp::{
    cluster::set_routing,
    protocol::{McpContentItem, McpToolCallResult},
    sync::clear_sync_cache,
    tools::execute_tool,
};
use prod_code_protocol::{
    HandshakeResponse, HostResources, ProdCodeCodec, StatusResponse, SyncProbeResponse,
    SyncResponse, WireMessage, negotiate_protocol_version,
};
use serde_json::{Value, json};
use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;
use url::Url;

fn make_workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    clear_sync_cache(&root);
    for (rel, content) in files {
        let full = root.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, content).unwrap();
    }
    (temp, root)
}

fn parse_report(result: &McpToolCallResult) -> Value {
    assert!(!result.is_error, "unexpected tool error: {result:?}");
    let McpContentItem::Text { text } = &result.content[0];
    serde_json::from_str(text).expect("valid workflow JSON report")
}

fn file_uri(path: &Path) -> String {
    Url::from_file_path(path).expect("file URI").to_string()
}

fn ws_sym(name: &str, kind: u64, uri: &str, line0: u32, col0: u32) -> Value {
    let end = col0 + name.len() as u32;
    json!({
        "name": name,
        "kind": kind,
        "location": {
            "uri": uri,
            "range": {"start": {"line": line0, "character": col0}, "end": {"line": line0, "character": end}}
        }
    })
}

fn doc_sym(name: &str, kind: u64, line0: u32, col0: u32, end_line0: u32, children: Value) -> Value {
    let end = col0 + name.len() as u32;
    json!({
        "name": name,
        "kind": kind,
        "range": {"start": {"line": line0, "character": 0}, "end": {"line": end_line0, "character": 1}},
        "selectionRange": {"start": {"line": line0, "character": col0}, "end": {"line": line0, "character": end}},
        "children": children
    })
}

fn make_status(pid: u32, engine: &str) -> StatusResponse {
    StatusResponse {
        server_pid: pid,
        uptime_seconds: 1,
        active_sessions: 0,
        loaded_workspaces: 0,
        detected_engines: vec![engine.into()],
        memory_rss_bytes: None,
        total_queries: 0,
        active_queries: 0,
        load_average_millis: Some(100),
        cpu_count: Some(4),
        platform: Some("linux x86_64".into()),
        running_commands: Vec::new(),
        host: HostResources::default(),
        version: None,
        git_commit: None,
    }
}

async fn start_rust_only_status_node() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let non_status = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&non_status);
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(msg)) = framed.next().await {
                    if matches!(msg, WireMessage::StatusRequest) {
                        let _ = framed
                            .send(WireMessage::StatusResponse(make_status(1, "rust")))
                            .await;
                    } else {
                        counter.fetch_add(1, Ordering::SeqCst);
                        let _ = framed
                            .send(WireMessage::Disconnect {
                                reason: "wrong engine node".into(),
                            })
                            .await;
                        return;
                    }
                }
            });
        }
    });
    (addr, non_status)
}

fn python_lsp_reply(py_uri: &str, method: &str) -> Value {
    match method {
        "initialize" => {
            json!({"capabilities": {"definitionProvider": true, "documentSymbolProvider": true, "workspaceSymbolProvider": true}})
        }
        "workspace/symbol" => json!([ws_sym("Worker", 5, py_uri, 0, 6)]),
        "textDocument/documentSymbol" => json!([doc_sym(
            "Worker",
            5,
            0,
            6,
            2,
            json!([doc_sym("run", 6, 1, 8, 2, json!([]))])
        )]),
        "textDocument/definition" => {
            json!({"uri": py_uri, "range": {"start": {"line": 1, "character": 8}, "end": {"line": 1, "character": 11}}})
        }
        _ => json!(null),
    }
}

async fn start_python_only_gateway(sample_py_uri: String) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let py_uri = sample_py_uri.clone();
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(msg)) = framed.next().await {
                    let reply = match msg {
                        WireMessage::StatusRequest => {
                            Some(WireMessage::StatusResponse(make_status(2, "python")))
                        }
                        WireMessage::SyncProbeRequest(req) => {
                            Some(WireMessage::SyncProbeResponse(SyncProbeResponse {
                                server_workspace_root: req.client_workspace_root,
                                seeded: false,
                                files_deleted: 0,
                                missing: Vec::new(),
                            }))
                        }
                        WireMessage::SyncRequest(req) => {
                            Some(WireMessage::SyncResponse(SyncResponse {
                                server_workspace_root: req.client_workspace_root,
                                files_updated: 0,
                                files_deleted: 0,
                                bytes_transferred: 0,
                                duration_ms: 0,
                                workspace_was_fresh: true,
                                stale_paths: Vec::new(),
                            }))
                        }
                        WireMessage::HandshakeRequest(req) => {
                            Some(WireMessage::HandshakeResponse(HandshakeResponse {
                                protocol_version: negotiate_protocol_version(&req).unwrap(),
                                server_pid: 2,
                                session_id: 1,
                                server_workspace_root: req.client_workspace_root,
                                detected_engine: "python".into(),
                                stale_paths: Vec::new(),
                                engine_age_ms: Some(10_000),
                                index_gated: false,
                                capabilities: None,
                            }))
                        }
                        WireMessage::LspPayload(raw) => {
                            serde_json::from_str::<Value>(&raw).ok().and_then(|val| {
                                let id = val.get("id")?.clone();
                                let method =
                                    val.get("method").and_then(Value::as_str).unwrap_or("");
                                let resp = json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": python_lsp_reply(&py_uri, method),
                                });
                                Some(WireMessage::LspPayload(resp.to_string()))
                            })
                        }
                        WireMessage::Disconnect { .. } => break,
                        _ => None,
                    };
                    if let Some(reply) = reply {
                        let _ = framed.send(reply).await;
                    }
                }
            });
        }
    });
    addr
}

#[tokio::test]
async fn workflow_resolve_and_symbols_route_cross_language_paths_through_cluster() {
    static CLUSTER_INIT: OnceLock<()> = OnceLock::new();
    let (_temp, root) = make_workspace(&[
        (
            "Cargo.toml",
            "[package]\nname = \"mixed_probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        ),
        ("src/lib.rs", "pub fn rust_fn() {}\n"),
        (
            "sample.py",
            "class Worker:\n    def run(self) -> int:\n        return 1\n",
        ),
        ("swift/Package.swift", "// swift-tools-version: 5.9\n"),
        ("swift/Sources/App/main.swift", "struct App {}\n"),
    ]);

    let (rust_only, rust_non_status) = start_rust_only_status_node().await;
    let python_only = start_python_only_gateway(file_uri(&root.join("sample.py"))).await;
    CLUSTER_INIT.get_or_init(|| set_routing(vec![python_only], "mixed-routing-fixture".into()));

    let py_resolve = execute_tool(
        rust_only,
        &root,
        "code_workflow",
        json!({"symbol": "Worker.run", "path": "sample.py", "include": ["body"]}),
    )
    .await
    .unwrap();
    let report = parse_report(&py_resolve);
    assert_eq!(report["status"], "complete", "{report}");
    assert!(
        report["steps"][1]["value"]["text"]
            .as_str()
            .unwrap()
            .contains("def run"),
        "{report}"
    );
    assert_eq!(rust_non_status.load(Ordering::SeqCst), 0);

    let py_symbols = execute_tool(
        rust_only,
        &root,
        "code_workflow",
        json!({"steps": [{"id": "s", "tool": "symbols", "args": {"query": "Worker.run", "file": "sample.py"}}]}),
    )
    .await
    .unwrap();
    let py_sym_report = parse_report(&py_symbols);
    assert_eq!(py_sym_report["status"], "complete", "{py_sym_report}");
    assert_eq!(
        py_sym_report["steps"][0]["value"]["items"][0]["name"], "run",
        "{py_sym_report}"
    );
    assert_eq!(rust_non_status.load(Ordering::SeqCst), 0);

    for step in [
        json!({"id": "s", "tool": "symbols", "args": {"query": "App", "path": "swift/Sources/App/main.swift"}}),
        json!({"id": "s", "tool": "resolve", "args": {"symbol": "App", "path": "swift/Sources/App/main.swift"}}),
    ] {
        let swift_result =
            execute_tool(rust_only, &root, "code_workflow", json!({"steps": [step]}))
                .await
                .unwrap();
        let swift_report = parse_report(&swift_result);
        assert_eq!(
            swift_report["steps"][0]["status"], "error",
            "{swift_report}"
        );
        assert!(
            swift_report["steps"][0]["error"]
                .as_str()
                .unwrap()
                .contains("serves swift"),
            "{swift_report}"
        );
        assert_eq!(rust_non_status.load(Ordering::SeqCst), 0);
    }

    let grep_swift = execute_tool(
        rust_only,
        &root,
        "code_workflow",
        json!({"steps": [{"id": "g", "tool": "grep", "args": {"pattern": "struct App", "path": "swift/Sources/App/main.swift"}}]}),
    )
    .await
    .unwrap();
    assert_eq!(parse_report(&grep_swift)["status"], "complete");
    assert_eq!(rust_non_status.load(Ordering::SeqCst), 0);
    clear_sync_cache(&root);
}
