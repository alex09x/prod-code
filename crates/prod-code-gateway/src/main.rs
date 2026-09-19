//! prod-code gateway daemon: multi-tenant server for remote code intelligence over 10 GbE LAN.

pub mod backend;
pub mod detect;
pub mod memory;
pub mod workspace;

pub use detect::detect_engine;

use anyhow::Result;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeResponse, PROTOCOL_VERSION, PathTranslator, ProdCodeCodec, StatusResponse, WireMessage,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;
use workspace::{SessionView, WorkspaceManager};

pub static NEXT_REQ_ID: AtomicU64 = AtomicU64::new(1);
pub static ACTIVE_QUERIES: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_QUERIES: AtomicU64 = AtomicU64::new(0);
pub static SLOW_QUERIES: AtomicU64 = AtomicU64::new(0);

#[derive(Parser, Debug)]
#[command(
    name = "prod-code-server",
    author,
    version,
    about = "Remote Code Intelligence Gateway"
)]
pub struct ServerCli {
    /// Bind address (IP:port). Defaults to 0.0.0.0:9400.
    #[arg(short, long, env = "PROD_CODE_BIND", default_value = "0.0.0.0:9400")]
    pub bind: SocketAddr,

    /// Workspace root storage directory on server.
    #[arg(
        short,
        long,
        env = "PROD_CODE_STORAGE",
        default_value = "/srv/prod-code/workspaces"
    )]
    pub storage: PathBuf,
}

pub struct ServerState {
    pub start_time: Instant,
    pub server_pid: u32,
    pub next_session_id: AtomicU64,
    pub active_sessions: AtomicUsize,
    pub storage_root: PathBuf,
    pub workspace_manager: Arc<WorkspaceManager>,
}

impl ServerState {
    pub fn new(storage_root: PathBuf) -> Self {
        Self {
            start_time: Instant::now(),
            server_pid: std::process::id(),
            next_session_id: AtomicU64::new(1),
            active_sessions: AtomicUsize::new(0),
            storage_root,
            workspace_manager: Arc::new(WorkspaceManager::new()),
        }
    }

    pub async fn status(&self) -> StatusResponse {
        StatusResponse {
            server_pid: self.server_pid,
            uptime_seconds: self.start_time.elapsed().as_secs(),
            active_sessions: self.active_sessions.load(Ordering::Relaxed),
            loaded_workspaces: self.workspace_manager.loaded_count().await,
            detected_engines: vec![
                "rust (ra_ap_ide)".to_string(),
                "go (gopls)".to_string(),
                "generic-lsp".to_string(),
            ],
            memory_rss_bytes: memory::get_process_rss_bytes(),
            total_queries: TOTAL_QUERIES.load(Ordering::Relaxed),
            active_queries: ACTIVE_QUERIES.load(Ordering::Relaxed),
        }
    }
}

pub async fn handle_client(
    socket: TcpStream,
    addr: SocketAddr,
    state: Arc<ServerState>,
) -> Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());

    while let Some(msg_res) = framed.next().await {
        let msg = msg_res?;
        match msg {
            WireMessage::StatusRequest => {
                let status = state.status().await;
                framed.send(WireMessage::StatusResponse(status)).await?;
            }
            WireMessage::Ping => {
                framed.send(WireMessage::Pong).await?;
            }
            WireMessage::HandshakeRequest(req) => {
                let session_id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
                state.active_sessions.fetch_add(1, Ordering::Relaxed);

                let client_root_path = PathBuf::from(&req.client_workspace_root);
                let folder_name = client_root_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("default");

                // Discover server workspace:
                // 1. Direct match in storage_root
                // 2. Home directory ~/folder_name (e.g. /home/alex09x/prod-code)
                // 3. ~/Projects/folder_name
                // 4. Default to storage_root/folder_name
                let mut server_workspace = state.storage_root.join(folder_name);
                if !server_workspace.exists() {
                    if let Some(home) = std::env::var("HOME").ok().map(PathBuf::from) {
                        let home_candidate = home.join(folder_name);
                        let projects_candidate = home.join("Projects").join(folder_name);
                        if home_candidate.exists() {
                            server_workspace = home_candidate;
                        } else if projects_candidate.exists() {
                            server_workspace = projects_candidate;
                        } else {
                            let _ = tokio::fs::create_dir_all(&server_workspace).await;
                        }
                    } else {
                        let _ = tokio::fs::create_dir_all(&server_workspace).await;
                    }
                }
                let server_workspace_str = server_workspace.to_string_lossy().to_string();

                let engine_kind =
                    detect::resolve_engine(&server_workspace, req.preferred_engine.as_deref());
                let engine = engine_kind.as_str();
                let translator =
                    PathTranslator::new(&req.client_workspace_root, &server_workspace_str);

                // Attach to shared workspace using leader-follower coalescing
                let shared_ws = state
                    .workspace_manager
                    .get_or_load(&server_workspace, engine)
                    .await?;

                let session_view = state
                    .workspace_manager
                    .register_session_view(session_id, client_root_path.clone(), shared_ws)
                    .await;

                tracing::info!(
                    session_id,
                    client_pid = req.client_pid,
                    client_root = %req.client_workspace_root,
                    server_root = %server_workspace_str,
                    engine,
                    is_single_owner = session_view.is_single_owner,
                    "Client session established (Direct-Edit fast path active: {})",
                    session_view.is_single_owner
                );

                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version: PROTOCOL_VERSION,
                        server_pid: state.server_pid,
                        session_id,
                        server_workspace_root: server_workspace_str,
                        detected_engine: engine.to_string(),
                    }))
                    .await?;

                // Session loop for streaming LSP and control messages
                let session_res = run_session_loop(&mut framed, &translator, &session_view).await;

                state
                    .workspace_manager
                    .unregister_session_view(&session_view)
                    .await;
                state.active_sessions.fetch_sub(1, Ordering::Relaxed);

                tracing::info!(session_id, "Client session retired: {:?}", session_res);
                return session_res;
            }
            WireMessage::Disconnect { reason } => {
                tracing::info!(%addr, reason, "Client disconnected cleanly");
                break;
            }
            other => {
                tracing::warn!(%addr, ?other, "Unexpected message before handshake");
            }
        }
    }

    Ok(())
}

async fn run_session_loop(
    framed: &mut Framed<TcpStream, ProdCodeCodec>,
    translator: &PathTranslator,
    view: &SessionView,
) -> Result<()> {
    let mut backend_rx = if let Some(ref go) = view.workspace.go_engine {
        Some(go.subscribe())
    } else if let Some(ref generic_eng) = view.workspace.generic_engine {
        Some(generic_eng.subscribe())
    } else {
        view.workspace.backend.as_ref().map(|b| b.subscribe())
    };

    loop {
        tokio::select! {
            client_msg_res = framed.next() => {
                match client_msg_res {
                    Some(Ok(WireMessage::Ping)) => {
                        framed.send(WireMessage::Pong).await?;
                    }
                    Some(Ok(WireMessage::LspPayload(raw_client_lsp))) => {
                        let server_lsp = translator.translate_lsp_to_server(&raw_client_lsp);
                        tracing::debug!(
                            payload_len = server_lsp.len(),
                            single_owner = view.is_single_owner,
                            "Processing incoming LSP message"
                        );

                        // Inspect LSP message structure
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&server_lsp) {
                            let method = val.get("method").and_then(|m| m.as_str());
                            let id = val.get("id").cloned();

                            // 1. Intercept "initialize": reply immediately with cached server capabilities
                            if method == Some("initialize") {
                                let req_id = id.unwrap_or(serde_json::json!(1));
                                let caps = if let Some(ref go) = view.workspace.go_engine {
                                    go.capabilities.read().await.clone()
                                } else if let Some(ref generic_eng) = view.workspace.generic_engine {
                                    generic_eng.capabilities.read().await.clone()
                                } else if let Some(ref backend) = view.workspace.backend {
                                    backend.capabilities.read().await.clone()
                                } else {
                                    None
                                };
                                let init_resp = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "result": {
                                        "capabilities": caps.unwrap_or_else(|| serde_json::json!({
                                            "textDocumentSync": 1,
                                            "hoverProvider": true,
                                            "definitionProvider": true,
                                            "referencesProvider": true,
                                            "documentSymbolProvider": true,
                                            "workspaceSymbolProvider": true
                                        })),
                                        "serverInfo": {
                                            "name": "prod-code-rci",
                                            "version": "0.1.0"
                                        }
                                    }
                                });
                                let client_resp = translator.translate_lsp_to_client(&init_resp.to_string());
                                framed.send(WireMessage::LspPayload(client_resp)).await?;
                                continue;
                            }

                            // 2. Intercept "initialized": backend already initialized, consume without forwarding
                            if method == Some("initialized") {
                                continue;
                            }

                            // 3. Intercept "shutdown": reply cleanly
                            if method == Some("shutdown") {
                                let req_id = id.unwrap_or(serde_json::json!(1));
                                let shutdown_resp = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "result": null
                                });
                                framed.send(WireMessage::LspPayload(shutdown_resp.to_string())).await?;
                                continue;
                            }

                            // 4. In-Memory RustEngine multi-core fast path: hover, definition, references, documentSymbol
                            if let Some(ref engine_lock) = view.workspace.rust_engine {
                                match method {
                                    Some("textDocument/hover") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let line = params.get("position").and_then(|p| p.get("line")).and_then(|l| l.as_u64()).unwrap_or(0) as u32;
                                            let col = params.get("position").and_then(|p| p.get("character")).and_then(|c| c.as_u64()).unwrap_or(0) as u32;
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));

                                            let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                            let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                            TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                            let query_start = Instant::now();

                                            tracing::info!(
                                                req = req_num,
                                                session = view.session_id,
                                                method = "textDocument/hover",
                                                file = %file_path.display(),
                                                pos = format!("{}:{}", line + 1, col + 1),
                                                in_flight,
                                                "🚀 [LSP START]"
                                            );

                                            // Acquire cheap snapshot (<1 µs) without holding mutex during query
                                            let snapshot = {
                                                let engine = engine_lock.lock().await;
                                                engine.snapshot()
                                            };

                                            let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                            let fp_clone = file_path.clone();
                                            let hover_res = tokio::task::spawn_blocking(move || {
                                                snapshot.hover(&fp_clone, line + 1, col + 1).ok().flatten()
                                            })
                                            .await
                                            .unwrap_or(None);

                                            let duration = query_start.elapsed();
                                            let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                            let ms = duration.as_secs_f64() * 1000.0;
                                            let found = hover_res.is_some();

                                            if ms > 200.0 {
                                                SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                tracing::warn!(
                                                    req = req_num,
                                                    session = view.session_id,
                                                    method = "textDocument/hover",
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    found,
                                                    in_flight = remaining,
                                                    "⚠️ [LSP SLOW >200ms]"
                                                );
                                            } else {
                                                tracing::info!(
                                                    req = req_num,
                                                    session = view.session_id,
                                                    method = "textDocument/hover",
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    found,
                                                    in_flight = remaining,
                                                    "✅ [LSP DONE]"
                                                );
                                            }

                                            let resp = match hover_res {
                                                Some(markup) => serde_json::json!({
                                                    "jsonrpc": "2.0",
                                                    "id": req_id,
                                                    "result": {
                                                        "contents": {
                                                            "kind": "markdown",
                                                            "value": markup
                                                        }
                                                    }
                                                }),
                                                None => serde_json::json!({
                                                    "jsonrpc": "2.0",
                                                    "id": req_id,
                                                    "result": null
                                                }),
                                            };
                                            let client_resp = translator.translate_lsp_to_client(&resp.to_string());
                                            framed.send(WireMessage::LspPayload(client_resp)).await?;
                                            continue;
                                        }
                                    }
                                    Some("textDocument/definition") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let line = params.get("position").and_then(|p| p.get("line")).and_then(|l| l.as_u64()).unwrap_or(0) as u32;
                                            let col = params.get("position").and_then(|p| p.get("character")).and_then(|c| c.as_u64()).unwrap_or(0) as u32;
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));

                                            let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                            let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                            TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                            let query_start = Instant::now();

                                            tracing::info!(
                                                req = req_num,
                                                session = view.session_id,
                                                method = "textDocument/definition",
                                                file = %file_path.display(),
                                                pos = format!("{}:{}", line + 1, col + 1),
                                                in_flight,
                                                "🚀 [LSP START]"
                                            );

                                            let snapshot = {
                                                let engine = engine_lock.lock().await;
                                                engine.snapshot()
                                            };

                                            let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                            let fp_clone = file_path.clone();
                                            let defs = tokio::task::spawn_blocking(move || {
                                                snapshot.goto_definition(&fp_clone, line + 1, col + 1).unwrap_or_default()
                                            })
                                            .await
                                            .unwrap_or_default();

                                            let duration = query_start.elapsed();
                                            let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                            let ms = duration.as_secs_f64() * 1000.0;
                                            let count = defs.len();

                                            if ms > 200.0 {
                                                SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                tracing::warn!(
                                                    req = req_num,
                                                    session = view.session_id,
                                                    method = "textDocument/definition",
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    targets = count,
                                                    in_flight = remaining,
                                                    "⚠️ [LSP SLOW >200ms]"
                                                );
                                            } else {
                                                tracing::info!(
                                                    req = req_num,
                                                    session = view.session_id,
                                                    method = "textDocument/definition",
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    targets = count,
                                                    in_flight = remaining,
                                                    "✅ [LSP DONE]"
                                                );
                                            }

                                            let locations: Vec<_> = defs.into_iter().map(|t| {
                                                serde_json::json!({
                                                    "uri": format!("file://{}", t.path.display()),
                                                    "range": {
                                                        "start": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) },
                                                        "end": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) }
                                                    }
                                                })
                                            }).collect();

                                            let resp = serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": req_id,
                                                "result": locations
                                            });
                                            let client_resp = translator.translate_lsp_to_client(&resp.to_string());
                                            framed.send(WireMessage::LspPayload(client_resp)).await?;
                                            continue;
                                        }
                                    }
                                    Some("textDocument/references") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let line = params.get("position").and_then(|p| p.get("line")).and_then(|l| l.as_u64()).unwrap_or(0) as u32;
                                            let col = params.get("position").and_then(|p| p.get("character")).and_then(|c| c.as_u64()).unwrap_or(0) as u32;
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));

                                            let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                            let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                            TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                            let query_start = Instant::now();

                                            tracing::info!(
                                                req = req_num,
                                                session = view.session_id,
                                                method = "textDocument/references",
                                                file = %file_path.display(),
                                                pos = format!("{}:{}", line + 1, col + 1),
                                                in_flight,
                                                "🚀 [LSP START]"
                                            );

                                            let snapshot = {
                                                let engine = engine_lock.lock().await;
                                                engine.snapshot()
                                            };

                                            let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                            let fp_clone = file_path.clone();
                                            let refs = tokio::task::spawn_blocking(move || {
                                                snapshot.find_all_refs(&fp_clone, line + 1, col + 1).unwrap_or_default()
                                            })
                                            .await
                                            .unwrap_or_default();

                                            let duration = query_start.elapsed();
                                            let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                            let ms = duration.as_secs_f64() * 1000.0;
                                            let count = refs.len();

                                            if ms > 200.0 {
                                                SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                tracing::warn!(
                                                    req = req_num,
                                                    session = view.session_id,
                                                    method = "textDocument/references",
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    references = count,
                                                    in_flight = remaining,
                                                    "⚠️ [LSP SLOW >200ms]"
                                                );
                                            } else {
                                                tracing::info!(
                                                    req = req_num,
                                                    session = view.session_id,
                                                    method = "textDocument/references",
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    references = count,
                                                    in_flight = remaining,
                                                    "✅ [LSP DONE]"
                                                );
                                            }

                                            let locations: Vec<_> = refs.into_iter().map(|t| {
                                                serde_json::json!({
                                                    "uri": format!("file://{}", t.path.display()),
                                                    "range": {
                                                        "start": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) },
                                                        "end": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) }
                                                    }
                                                })
                                            }).collect();

                                            let resp = serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": req_id,
                                                "result": locations
                                            });
                                            let client_resp = translator.translate_lsp_to_client(&resp.to_string());
                                            framed.send(WireMessage::LspPayload(client_resp)).await?;
                                            continue;
                                        }
                                    }
                                    Some("textDocument/documentSymbol") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));

                                            let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                            let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                            TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                            let query_start = Instant::now();

                                            tracing::info!(
                                                req = req_num,
                                                session = view.session_id,
                                                method = "textDocument/documentSymbol",
                                                file = %file_path.display(),
                                                in_flight,
                                                "🚀 [LSP START]"
                                            );

                                            let snapshot = {
                                                let engine = engine_lock.lock().await;
                                                engine.snapshot()
                                            };

                                            let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                            let fp_clone = file_path.clone();
                                            let syms = tokio::task::spawn_blocking(move || {
                                                snapshot.document_symbols(&fp_clone).unwrap_or_default()
                                            })
                                            .await
                                            .unwrap_or_default();

                                            let duration = query_start.elapsed();
                                            let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                            let ms = duration.as_secs_f64() * 1000.0;
                                            let count = syms.len();

                                            if ms > 200.0 {
                                                SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                tracing::warn!(
                                                    req = req_num,
                                                    session = view.session_id,
                                                    method = "textDocument/documentSymbol",
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    symbols = count,
                                                    in_flight = remaining,
                                                    "⚠️ [LSP SLOW >200ms]"
                                                );
                                            } else {
                                                tracing::info!(
                                                    req = req_num,
                                                    session = view.session_id,
                                                    method = "textDocument/documentSymbol",
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    symbols = count,
                                                    in_flight = remaining,
                                                    "✅ [LSP DONE]"
                                                );
                                            }

                                            let sym_list: Vec<_> = syms.into_iter().map(|s| {
                                                let kind_num = match s.kind.as_str() {
                                                    "Fn" | "Function" => 12,
                                                    "Struct" => 23,
                                                    "Enum" => 10,
                                                    "Const" | "Constant" => 14,
                                                    "Trait" => 11,
                                                    "Module" => 2,
                                                    _ => 13,
                                                };
                                                serde_json::json!({
                                                    "name": s.name,
                                                    "kind": kind_num,
                                                    "location": {
                                                        "uri": uri,
                                                        "range": {
                                                            "start": { "line": s.line.saturating_sub(1), "character": 0 },
                                                            "end": { "line": s.line.saturating_sub(1), "character": 0 }
                                                        }
                                                    },
                                                    "containerName": s.detail
                                                })
                                            }).collect();

                                            let resp = serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": req_id,
                                                "result": sym_list
                                            });
                                            let client_resp = translator.translate_lsp_to_client(&resp.to_string());
                                            framed.send(WireMessage::LspPayload(client_resp)).await?;
                                            continue;
                                        }
                                    }
                                    Some("textDocument/didOpen") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));
                                            if let Some(text) = params.get("textDocument").and_then(|td| td.get("text")).and_then(|t| t.as_str()) {
                                                let edit_start = Instant::now();
                                                let text_len = text.len();
                                                {
                                                    let mut engine = engine_lock.lock().await;
                                                    let _ = engine.apply_file_change(&file_path, text.to_string());
                                                }
                                                let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                                tracing::info!(
                                                    session = view.session_id,
                                                    file = %file_path.display(),
                                                    bytes = text_len,
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    "📝 [DIRECT-EDIT] applied didOpen directly into Salsa DB in RAM"
                                                );
                                            }
                                        }
                                    }
                                    Some("textDocument/didChange") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));
                                            let first = params
                                                .get("contentChanges")
                                                .and_then(|c| c.as_array())
                                                .and_then(|arr| arr.first())
                                                .and_then(|c| c.get("text"))
                                                .and_then(|t| t.as_str());
                                            if let Some(text) = first {
                                                let edit_start = Instant::now();
                                                let text_len = text.len();
                                                {
                                                    let mut engine = engine_lock.lock().await;
                                                    let _ = engine.apply_file_change(&file_path, text.to_string());
                                                }
                                                let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                                tracing::info!(
                                                    session = view.session_id,
                                                    file = %file_path.display(),
                                                    bytes = text_len,
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    "📝 [DIRECT-EDIT] applied didChange directly into Salsa DB in RAM"
                                                );
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }

                            // 5. Fallback handling for textDocument/didOpen vs didChange on backend worker
                            if let (Some("textDocument/didOpen"), Some(backend)) = (method, &view.workspace.backend) {
                                let uri = val.get("params")
                                    .and_then(|p| p.get("textDocument"))
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let is_open = backend.open_files.read().await.contains(uri);
                                if is_open {
                                    let text = val.get("params")
                                        .and_then(|p| p.get("textDocument"))
                                        .and_then(|td| td.get("text"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let version = val.get("params")
                                        .and_then(|p| p.get("textDocument"))
                                        .and_then(|td| td.get("version"))
                                        .and_then(|v| v.as_i64())
                                        .unwrap_or(2);
                                    let did_change = serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "method": "textDocument/didChange",
                                        "params": {
                                            "textDocument": {
                                                "uri": uri,
                                                "version": version
                                            },
                                            "contentChanges": [
                                                { "text": text }
                                            ]
                                        }
                                    });
                                    let _ = backend.send_lsp(&did_change.to_string()).await;
                                    continue;
                                } else {
                                    backend.open_files.write().await.insert(uri.to_string());
                                }
                            }

                            // 5b. Supervised GoEngine fast path
                            if let Some(ref go) = view.workspace.go_engine {
                                match method {
                                    Some("textDocument/didOpen") => {
                                        let uri = val.get("params").and_then(|p| p.get("textDocument")).and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                        let text = val.get("params").and_then(|p| p.get("textDocument")).and_then(|td| td.get("text")).and_then(|t| t.as_str()).unwrap_or("");
                                        let _ = go.did_open(uri, text).await;
                                        continue;
                                    }
                                    Some("textDocument/didChange") => {
                                        let uri = val.get("params").and_then(|p| p.get("textDocument")).and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                        let version = val.get("params").and_then(|p| p.get("textDocument")).and_then(|td| td.get("version")).and_then(|v| v.as_i64()).unwrap_or(1) as i32;
                                        let text = val.get("params").and_then(|p| p.get("contentChanges")).and_then(|c| c.as_array()).and_then(|a| a.first()).and_then(|ch| ch.get("text")).and_then(|t| t.as_str()).unwrap_or("");
                                        let _ = go.did_change(uri, text, version).await;
                                        continue;
                                    }
                                    Some("textDocument/didClose") => {
                                        let uri = val.get("params").and_then(|p| p.get("textDocument")).and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                        let _ = go.did_close(uri).await;
                                        continue;
                                    }
                                    Some(m) if id.is_some() => {
                                        let req_id_log = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                        let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                        let start = Instant::now();

                                        tracing::info!(
                                            req = req_id_log,
                                            session = view.session_id,
                                            method = m,
                                            in_flight,
                                            "🚀 [LSP START] dispatching to GoEngine"
                                        );

                                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                                        let resp_res = go.send_request(m, params).await;
                                        let duration = start.elapsed();
                                        let duration_ms = duration.as_secs_f64() * 1000.0;
                                        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

                                        if duration_ms > 200.0 {
                                            SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                            tracing::warn!(
                                                req = req_id_log,
                                                session = view.session_id,
                                                method = m,
                                                duration_ms = %format!("{:.2}ms", duration_ms),
                                                in_flight = remaining,
                                                "⚠️ [LSP SLOW >200ms] GoEngine query exceeded threshold"
                                            );
                                        } else {
                                            tracing::info!(
                                                req = req_id_log,
                                                session = view.session_id,
                                                method = m,
                                                duration_ms = %format!("{:.2}ms", duration_ms),
                                                in_flight = remaining,
                                                "✅ [LSP DONE] GoEngine query complete"
                                            );
                                        }

                                        match resp_res {
                                            Ok(mut resp) => {
                                                if let Some(ref req_id) = id {
                                                    resp["id"] = req_id.clone();
                                                }
                                                let client_resp = translator.translate_lsp_to_client(&resp.to_string());
                                                framed.send(WireMessage::LspPayload(client_resp)).await?;
                                            }
                                            Err(err) => {
                                                let err_resp = serde_json::json!({
                                                    "jsonrpc": "2.0",
                                                    "id": id,
                                                    "error": { "code": -32603, "message": err.to_string() }
                                                });
                                                framed.send(WireMessage::LspPayload(err_resp.to_string())).await?;
                                            }
                                        }
                                        continue;
                                    }
                                    Some(m) => {
                                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                                        let _ = go.send_notification(m, params).await;
                                        continue;
                                    }
                                    None => {}
                                }
                            }

                            // 5c. Supervised GenericLspEngine fast path
                            if let Some(ref generic_eng) = view.workspace.generic_engine {
                                if let (Some(m), Some(req_id)) = (method, &id) {
                                    let req_id_log = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                    let start = Instant::now();

                                    tracing::info!(
                                        req = req_id_log,
                                        session = view.session_id,
                                        method = m,
                                        in_flight,
                                        "🚀 [LSP START] dispatching to GenericLspEngine"
                                    );

                                    let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                                    let resp_res = generic_eng.send_request(m, params).await;
                                    let duration = start.elapsed();
                                    let duration_ms = duration.as_secs_f64() * 1000.0;
                                    let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

                                    if duration_ms > 200.0 {
                                        SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                        tracing::warn!(
                                            req = req_id_log,
                                            session = view.session_id,
                                            method = m,
                                            duration_ms = %format!("{:.2}ms", duration_ms),
                                            in_flight = remaining,
                                            "⚠️ [LSP SLOW >200ms] GenericLspEngine query exceeded threshold"
                                        );
                                    } else {
                                        tracing::info!(
                                            req = req_id_log,
                                            session = view.session_id,
                                            method = m,
                                            duration_ms = %format!("{:.2}ms", duration_ms),
                                            in_flight = remaining,
                                            "✅ [LSP DONE] GenericLspEngine query complete"
                                        );
                                    }

                                    match resp_res {
                                        Ok(mut resp) => {
                                            resp["id"] = req_id.clone();
                                            let client_resp = translator.translate_lsp_to_client(&resp.to_string());
                                            framed.send(WireMessage::LspPayload(client_resp)).await?;
                                        }
                                        Err(err) => {
                                            let err_resp = serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": req_id,
                                                "error": { "code": -32603, "message": err.to_string() }
                                            });
                                            framed.send(WireMessage::LspPayload(err_resp.to_string())).await?;
                                        }
                                    }
                                    continue;
                                } else if let Some(m) = method {
                                    let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                                    let _ = generic_eng.send_notification(m, params).await;
                                    continue;
                                }
                            }

                            // 6. Handle "textDocument/didClose"
                            if let (Some("textDocument/didClose"), Some(backend)) = (method, &view.workspace.backend) {
                                let uri = val.get("params")
                                    .and_then(|p| p.get("textDocument"))
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                backend.open_files.write().await.remove(uri);
                            }

                            // 7. If no backend is attached and client expects a response, return empty result
                            if let (Some(req_id), None, None, None, None) = (
                                id,
                                &view.workspace.backend,
                                &view.workspace.rust_engine,
                                &view.workspace.go_engine,
                                &view.workspace.generic_engine,
                            ) {
                                let empty_resp = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "result": null
                                });
                                framed.send(WireMessage::LspPayload(empty_resp.to_string())).await?;
                                continue;
                            }
                        }

                        if let Some(backend) = &view.workspace.backend {
                            let _ = backend.send_lsp(&server_lsp).await.inspect_err(|e| {
                                tracing::error!(error = %e, "Failed to forward LSP to backend worker");
                            });
                        }
                    }
                    Some(Ok(WireMessage::Disconnect { reason })) => {
                        tracing::info!(reason, "Client terminated session");
                        break;
                    }
                    Some(Ok(WireMessage::StatusRequest)) => {
                        framed
                            .send(WireMessage::StatusResponse(StatusResponse {
                                server_pid: std::process::id(),
                                uptime_seconds: 0,
                                active_sessions: 1,
                                loaded_workspaces: 1,
                                detected_engines: vec![view.workspace.engine.clone()],
                                memory_rss_bytes: memory::get_process_rss_bytes(),
                                total_queries: TOTAL_QUERIES.load(Ordering::Relaxed),
                                active_queries: ACTIVE_QUERIES.load(Ordering::Relaxed),
                            }))
                            .await?;
                    }
                    Some(Err(e)) => {
                        tracing::error!(error = %e, "TCP frame decode error");
                        break;
                    }
                    None => {
                        tracing::info!("Client disconnected");
                        break;
                    }
                    _ => {}
                }
            }

            backend_msg = async {
                if let Some(ref mut rx) = backend_rx {
                    rx.recv().await
                } else {
                    futures_util::future::pending::<Result<String, tokio::sync::broadcast::error::RecvError>>().await
                }
            } => {
                match backend_msg {
                    Ok(server_lsp) => {
                        let client_lsp = translator.translate_lsp_to_client(&server_lsp);
                        if let Err(e) = framed.send(WireMessage::LspPayload(client_lsp)).await {
                            tracing::error!(error = %e, "Failed to send LSP message to client");
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "Session backend receiver lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::warn!("Backend worker broadcast closed");
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "info,prod_code_gateway=debug,prod_code_engine_rust=debug".into()
            }),
        )
        .init();
    let cli = ServerCli::parse();

    tracing::info!(
        "prod-code gateway daemon starting on {} (storage: {:?})",
        cli.bind,
        cli.storage
    );

    let state = Arc::new(ServerState::new(cli.storage));
    let listener = TcpListener::bind(cli.bind).await?;
    tracing::info!("prod-code gateway listening on {}", cli.bind);

    loop {
        let (socket, addr) = listener.accept().await?;
        let state_clone = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(err) = handle_client(socket, addr, state_clone).await {
                tracing::error!(%addr, %err, "Error in client connection");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_server_state_status() {
        let temp = tempfile::tempdir().unwrap();
        let state = ServerState::new(temp.path().to_path_buf());
        let status = state.status().await;
        assert_eq!(status.server_pid, std::process::id());
        assert_eq!(status.active_sessions, 0);
        assert_eq!(status.loaded_workspaces, 0);
        assert!(
            status
                .detected_engines
                .contains(&"rust (ra_ap_ide)".to_string())
        );
    }

    #[test]
    fn test_engine_detection() {
        use prod_code_protocol::messages::EngineKind;

        let temp = tempfile::tempdir().unwrap();
        assert_eq!(detect_engine(temp.path()), EngineKind::Generic);

        std::fs::write(temp.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_engine(temp.path()), EngineKind::Rust);

        let go_temp = tempfile::tempdir().unwrap();
        std::fs::write(go_temp.path().join("go.mod"), "").unwrap();
        assert_eq!(detect_engine(go_temp.path()), EngineKind::Go);

        let py_temp = tempfile::tempdir().unwrap();
        std::fs::write(py_temp.path().join("pyproject.toml"), "").unwrap();
        assert_eq!(detect_engine(py_temp.path()), EngineKind::Python);

        let ts_temp = tempfile::tempdir().unwrap();
        std::fs::write(ts_temp.path().join("package.json"), "").unwrap();
        assert_eq!(detect_engine(ts_temp.path()), EngineKind::TypeScript);
    }
}
