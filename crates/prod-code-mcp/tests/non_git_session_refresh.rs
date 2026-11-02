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
use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_mcp::tools::execute_tool;
use prod_code_mcp::watch::current_generation;
use prod_code_protocol::{
    HandshakeResponse, PROTOCOL_VERSION, ProdCodeCodec, SyncProbeResponse, SyncRequest,
    SyncResponse, WireMessage,
};
use prod_code_testkit::answers;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

const CARGO_TOML: &str =
    "[package]\nname = \"non_git_refresh_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const INITIAL_LIB: &str = "pub fn workflow_unique(value: u32) -> u32 { helper(value) + 1 }\n\
                           fn helper(value: u32) -> u32 { value * 2 }\n";

struct GatewayTelemetry {
    server_mirror: PathBuf,
    handshakes: AtomicUsize,
    post_handshake_probes: AtomicUsize,
    session_syncs: AtomicUsize,
}

fn text_of(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|McpContentItem::Text { text }| text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn apply_sync_to_mirror(mirror_root: &Path, request: &SyncRequest) -> (usize, usize, usize) {
    let (mut updated, mut deleted, mut bytes) = (0, 0, 0);
    for delta in &request.files {
        let target = mirror_root.join(&delta.relative_path);
        match &delta.content {
            Some(content) => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).expect("create mirror parent");
                }
                bytes += content.len();
                updated += 1;
                std::fs::write(&target, content).expect("write mirror file");
            }
            None if target.exists() => {
                std::fs::remove_file(&target).expect("remove mirror file");
                deleted += 1;
            }
            None => {}
        }
    }
    (updated, deleted, bytes)
}

fn scan_fns_in_mirror(mirror_root: &Path, client_root: &Path) -> Vec<(String, PathBuf, u32, u32)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(mirror_root.join("src")) else {
        return found;
    };
    for entry in entries.flatten() {
        let mirror_file = entry.path();
        if mirror_file.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let Ok(rel) = mirror_file.strip_prefix(mirror_root) else {
            continue;
        };
        let client_file = client_root.join(rel);
        let Ok(text) = std::fs::read_to_string(&mirror_file) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            for prefix in ["pub fn ", "fn "] {
                if let Some(rest) = line.trim_start().strip_prefix(prefix) {
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() {
                        let col = line.find(&name).map(|i| i as u32 + 1).unwrap_or(1);
                        found.push((name, client_file.clone(), idx as u32 + 1, col));
                    }
                    break;
                }
            }
        }
    }
    found
}

fn answer_lsp(req: &serde_json::Value, fns: &[(String, PathBuf, u32, u32)]) -> serde_json::Value {
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let uri_matches = |path: &Path, uri: &str| {
        url::Url::from_file_path(path)
            .map(|u| u.as_str() == uri)
            .unwrap_or(false)
    };
    match method {
        "initialize" => serde_json::json!({
            "capabilities": {
                "workspaceSymbolProvider": true,
                "documentSymbolProvider": true,
                "definitionProvider": true
            }
        }),
        "workspace/symbol" => {
            let query = req["params"]["query"].as_str().unwrap_or("");
            serde_json::Value::Array(
                fns.iter()
                    .filter(|(name, ..)| name.contains(query))
                    .map(|(name, p, l, c)| answers::symbol(name, 12, p, *l, *c))
                    .collect(),
            )
        }
        "textDocument/documentSymbol" => {
            let uri = req["params"]["textDocument"]["uri"].as_str().unwrap_or("");
            serde_json::Value::Array(
                fns.iter()
                    .filter(|(_, p, ..)| uri_matches(p, uri))
                    .map(|(name, _, l, c)| answers::document_symbol(name, 12, *l, *l, *c))
                    .collect(),
            )
        }
        "textDocument/definition" => {
            let uri = req["params"]["textDocument"]["uri"].as_str().unwrap_or("");
            let line = req["params"]["position"]["line"].as_u64().unwrap_or(0) as u32 + 1;
            match fns
                .iter()
                .find(|(_, p, fn_line, _)| *fn_line == line && uri_matches(p, uri))
            {
                Some((_, p, l, c)) => answers::locations(p, &[(*l, *c)]),
                None => serde_json::json!([]),
            }
        }
        _ => serde_json::Value::Null,
    }
}

async fn start_two_phase_gateway(
    client_root: PathBuf,
    telemetry: Arc<GatewayTelemetry>,
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("gateway addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let client_root = client_root.clone();
            let telemetry = Arc::clone(&telemetry);
            tokio::spawn(async move {
                let _ = serve_two_phase_connection(socket, &client_root, &telemetry).await;
            });
        }
    });
    addr
}

async fn serve_two_phase_connection(
    socket: TcpStream,
    client_root: &Path,
    telemetry: &GatewayTelemetry,
) -> anyhow::Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    let mut handshaken = false;
    let mirror_str = telemetry.server_mirror.to_string_lossy().into_owned();
    while let Some(message) = framed.next().await {
        match message? {
            WireMessage::SyncProbeRequest(req) if !handshaken => {
                let missing = req.files.into_iter().map(|e| e.relative_path).collect();
                framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: mirror_str.clone(),
                        seeded: false,
                        files_deleted: 0,
                        missing,
                    }))
                    .await?;
            }
            WireMessage::SyncProbeRequest(_) => {
                telemetry
                    .post_handshake_probes
                    .fetch_add(1, Ordering::SeqCst);
            }
            WireMessage::SyncRequest(request) => {
                if handshaken {
                    telemetry.session_syncs.fetch_add(1, Ordering::SeqCst);
                }
                let (files_updated, files_deleted, bytes_transferred) =
                    apply_sync_to_mirror(&telemetry.server_mirror, &request);
                framed
                    .send(WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: mirror_str.clone(),
                        files_updated,
                        files_deleted,
                        bytes_transferred,
                        duration_ms: 1,
                        workspace_was_fresh: false,
                        stale_paths: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::HandshakeRequest(_) if !handshaken => {
                handshaken = true;
                telemetry.handshakes.fetch_add(1, Ordering::SeqCst);
                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version: PROTOCOL_VERSION,
                        server_pid: std::process::id(),
                        session_id: 1,
                        server_workspace_root: mirror_str.clone(),
                        detected_engine: "rust".to_string(),
                        stale_paths: Vec::new(),
                        engine_age_ms: Some(5_000),
                        index_gated: true,
                        capabilities: None,
                    }))
                    .await?;
            }
            WireMessage::LspPayload(payload) if handshaken => {
                let Ok(req) = serde_json::from_str::<serde_json::Value>(&payload) else {
                    continue;
                };
                let Some(id) = req.get("id").cloned() else {
                    continue;
                };
                let fns = scan_fns_in_mirror(&telemetry.server_mirror, client_root);
                let result = answer_lsp(&req, &fns);
                framed
                    .send(WireMessage::LspPayload(
                        serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
                            .to_string(),
                    ))
                    .await?;
            }
            WireMessage::Disconnect { .. } => break,
            _ => {}
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_git_session_queries_and_refreshes_real_edits_without_post_handshake_probe() {
    let client_temp = tempfile::tempdir().expect("client tempdir");
    let client_root = std::fs::canonicalize(client_temp.path()).expect("canonical client root");
    std::fs::create_dir_all(client_root.join("src")).expect("create src");
    std::fs::write(client_root.join("Cargo.toml"), CARGO_TOML).expect("write Cargo.toml");
    std::fs::write(client_root.join("src/lib.rs"), INITIAL_LIB).expect("write lib.rs");
    std::fs::write(
        client_root.join("src/obsolete.rs"),
        "pub fn obsolete_symbol() -> u32 { 0 }\n",
    )
    .expect("write obsolete.rs");

    let mirror_temp = tempfile::tempdir().expect("mirror tempdir");
    let telemetry = Arc::new(GatewayTelemetry {
        server_mirror: mirror_temp.path().to_path_buf(),
        handshakes: AtomicUsize::new(0),
        post_handshake_probes: AtomicUsize::new(0),
        session_syncs: AtomicUsize::new(0),
    });
    let remote = start_two_phase_gateway(client_root.clone(), Arc::clone(&telemetry)).await;

    let sym = execute_tool(
        remote,
        &client_root,
        "code_symbols",
        serde_json::json!({ "query": "workflow_unique", "path": "src/lib.rs" }),
    )
    .await
    .expect("code_symbols succeeds");
    assert!(!sym.is_error, "{}", text_of(&sym));

    let def1 = execute_tool(
        remote,
        &client_root,
        "code_definition",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8, "budget_ms": 800 }),
    )
    .await
    .expect("initial code_definition succeeds");
    assert!(
        !def1.is_error && text_of(&def1).contains("workflow_unique"),
        "{}",
        text_of(&def1)
    );

    let gen_before = current_generation(&client_root);
    std::fs::write(
        client_root.join("src/lib.rs"),
        format!("{INITIAL_LIB}pub fn refreshed_symbol() -> u32 {{ 99 }}\n"),
    )
    .expect("modify lib.rs");
    std::fs::write(
        client_root.join("src/added.rs"),
        "pub fn newly_created() -> u32 { 7 }\n",
    )
    .expect("create added.rs");
    std::fs::remove_file(client_root.join("src/obsolete.rs")).expect("remove obsolete.rs");

    let wait_until = Instant::now() + Duration::from_secs(2);
    while current_generation(&client_root) == gen_before && Instant::now() < wait_until {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    for expected in ["refreshed_symbol", "newly_created"] {
        let def = execute_tool(
            remote,
            &client_root,
            "code_definition",
            serde_json::json!({ "symbol": expected, "budget_ms": 1_200 }),
        )
        .await
        .expect("refreshed definition");
        assert!(
            !def.is_error && text_of(&def).contains(expected),
            "{}",
            text_of(&def)
        );
    }

    assert_eq!(telemetry.post_handshake_probes.load(Ordering::SeqCst), 0);
    assert_eq!(telemetry.handshakes.load(Ordering::SeqCst), 1);
    assert!(telemetry.session_syncs.load(Ordering::SeqCst) >= 1);
    assert!(!telemetry.server_mirror.join("src/obsolete.rs").exists());
    assert!(telemetry.server_mirror.join("src/added.rs").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_sync_probe_timeout_is_not_misreported_as_lsp_analyzer_busy() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let remote = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });

    let temp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(temp.path()).expect("canonical root");
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(root.join("Cargo.toml"), CARGO_TOML).expect("Cargo.toml");
    std::fs::write(root.join("src/lib.rs"), INITIAL_LIB).expect("lib.rs");

    for tool in ["code_definition", "code_references"] {
        let result = execute_tool(
            remote,
            &root,
            tool,
            serde_json::json!({
                "path": "src/lib.rs",
                "line": 1,
                "character": 8,
                "budget_ms": 250
            }),
        )
        .await
        .expect("budgeted tool call returns structured error");
        let text = text_of(&result);
        assert!(result.is_error, "{tool}: {text}");
        assert!(
            text.contains("sync") || text.contains("probe"),
            "{tool} timeout must preserve the sync/probe stage: {text}"
        );
        assert!(
            !text.contains("the analyzer may be busy indexing or compiling"),
            "{tool} sync timeout must not blame the LSP analyzer: {text}"
        );
    }
}
