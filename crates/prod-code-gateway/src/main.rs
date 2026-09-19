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
    HandshakeResponse, PROTOCOL_VERSION, PathTranslator, ProdCodeCodec, StatusResponse,
    SyncRequest, SyncResponse, WireMessage,
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

/// Apply batch file synchronization to server workspace storage.
pub async fn apply_sync(storage_root: &std::path::Path, req: SyncRequest) -> SyncResponse {
    let start = Instant::now();
    let server_workspace = workspace::resolve_server_workspace(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let folder_name = server_workspace
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("default");

    let mut files_updated = 0;
    let mut files_deleted = 0;
    let mut bytes_transferred = 0;

    for delta in req.files {
        let target_path = server_workspace.join(&delta.relative_path);
        match delta.content {
            Some(content_bytes) => {
                if let Some(parent) = target_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                bytes_transferred += content_bytes.len();
                if tokio::fs::write(&target_path, &content_bytes).await.is_ok() {
                    files_updated += 1;
                }
            }
            None => {
                if target_path.exists() && tokio::fs::remove_file(&target_path).await.is_ok() {
                    files_deleted += 1;
                }
            }
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    tracing::info!(
        folder_name,
        files_updated,
        files_deleted,
        bytes_transferred,
        duration_ms = %format!("{duration_ms}ms"),
        "⚡ [SYNC] Workspace fast-sync applied"
    );

    SyncResponse {
        files_updated,
        files_deleted,
        bytes_transferred,
        duration_ms,
        server_workspace_root: server_workspace.to_string_lossy().to_string(),
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
            WireMessage::SyncRequest(req) => {
                let resp = apply_sync(&state.storage_root, req).await;
                framed.send(WireMessage::SyncResponse(resp)).await?;
            }
            WireMessage::Ping => {
                framed.send(WireMessage::Pong).await?;
            }
            WireMessage::HandshakeRequest(req) => {
                let session_id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
                state.active_sessions.fetch_add(1, Ordering::Relaxed);

                let client_root_path = PathBuf::from(&req.client_workspace_root);
                let server_workspace = workspace::resolve_server_workspace(
                    &state.storage_root,
                    &req.client_workspace_root,
                    req.base_workspace_name.as_deref(),
                );
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
                let session_res = run_session_loop(framed, &translator, &session_view).await;

                if let Some(engine_lock) = &session_view.workspace.rust_engine {
                    let mut engine = engine_lock.lock().await;
                    if let Err(e) = engine.clear_session(session_id) {
                        tracing::warn!(error = %e, session_id, "failed to drop session overlays");
                    }
                }

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
    framed: Framed<TcpStream, ProdCodeCodec>,
    translator: &PathTranslator,
    view: &SessionView,
) -> Result<()> {
    let (mut socket_tx, mut socket_rx) = framed.split();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<WireMessage>(4096);

    let writer_handle = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if socket_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut backend_rx = if let Some(ref go) = view.workspace.go_engine {
        Some(go.subscribe())
    } else if let Some(ref generic_eng) = view.workspace.generic_engine {
        Some(generic_eng.subscribe())
    } else {
        view.workspace.backend.as_ref().map(|b| b.subscribe())
    };

    loop {
        tokio::select! {
            client_msg_res = socket_rx.next() => {
                match client_msg_res {
                    Some(Ok(WireMessage::Ping)) => {
                        let _ = out_tx.send(WireMessage::Pong).await;
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
                                let _ = out_tx.send(WireMessage::LspPayload(client_resp)).await;
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
                                let _ = out_tx.send(WireMessage::LspPayload(shutdown_resp.to_string())).await;
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
                                            let engine_arc = Arc::clone(engine_lock);

                                            let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                            let fp_clone = file_path.clone();
                                            let out_tx_task = out_tx.clone();
                                            let translator_task = translator.clone();
                                            let session_id = view.session_id;

                                            tokio::task::spawn(async move {
                                                let hover_res = {
                                                    // Hold the engine for the whole query: activating the session view and
                                                    // running the query under one lock keeps other sessions' buffers out and
                                                    // prevents a concurrent edit from cancelling this snapshot.
                                                    let mut engine = engine_arc.lock_owned().await;
                                                    tokio::task::spawn_blocking(move || {
                                                        if let Err(e) = engine.activate_session(session_id) {
                                                            tracing::warn!(error = %e, session = session_id, "session view activation failed");
                                                        }
                                                        engine.hover(&fp_clone, line + 1, col + 1).unwrap_or_else(|e| {
                                                            tracing::warn!(error = %e, session = session_id, "query failed");
                                                            None
                                                        })
                                                    })
                                                    .await
                                                    .unwrap_or(None)
                                                };

                                                let duration = query_start.elapsed();
                                                let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                                let ms = duration.as_secs_f64() * 1000.0;
                                                let found = hover_res.is_some();

                                                if ms > 200.0 {
                                                    SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                    tracing::warn!(
                                                        req = req_num,
                                                        session = session_id,
                                                        method = "textDocument/hover",
                                                        duration_ms = format!("{:.2}ms", ms),
                                                        found,
                                                        in_flight = remaining,
                                                        "⚠️ [LSP SLOW >200ms]"
                                                    );
                                                } else {
                                                    tracing::info!(
                                                        req = req_num,
                                                        session = session_id,
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
                                                let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                            });
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

                                            let engine_arc = Arc::clone(engine_lock);

                                            let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                            let fp_clone = file_path.clone();
                                            let out_tx_task = out_tx.clone();
                                            let translator_task = translator.clone();
                                            let session_id = view.session_id;

                                            tokio::task::spawn(async move {
                                                let defs = {
                                                    // Hold the engine for the whole query: activating the session view and
                                                    // running the query under one lock keeps other sessions' buffers out and
                                                    // prevents a concurrent edit from cancelling this snapshot.
                                                    let mut engine = engine_arc.lock_owned().await;
                                                    tokio::task::spawn_blocking(move || {
                                                        if let Err(e) = engine.activate_session(session_id) {
                                                            tracing::warn!(error = %e, session = session_id, "session view activation failed");
                                                        }
                                                        engine.goto_definition(&fp_clone, line + 1, col + 1).unwrap_or_else(|e| {
                                                            tracing::warn!(error = %e, session = session_id, "query failed");
                                                            Vec::new()
                                                        })
                                                    })
                                                    .await
                                                    .unwrap_or_default()
                                                };

                                                let duration = query_start.elapsed();
                                                let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                                let ms = duration.as_secs_f64() * 1000.0;
                                                let count = defs.len();

                                                if ms > 200.0 {
                                                    SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                    tracing::warn!(
                                                        req = req_num,
                                                        session = session_id,
                                                        method = "textDocument/definition",
                                                        duration_ms = format!("{:.2}ms", ms),
                                                        targets = count,
                                                        in_flight = remaining,
                                                        "⚠️ [LSP SLOW >200ms]"
                                                    );
                                                } else {
                                                    tracing::info!(
                                                        req = req_num,
                                                        session = session_id,
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
                                                let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                            });
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

                                            let engine_arc = Arc::clone(engine_lock);

                                            let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                            let fp_clone = file_path.clone();
                                            let out_tx_task = out_tx.clone();
                                            let translator_task = translator.clone();
                                            let session_id = view.session_id;

                                            tokio::task::spawn(async move {
                                                let refs = {
                                                    // Hold the engine for the whole query: activating the session view and
                                                    // running the query under one lock keeps other sessions' buffers out and
                                                    // prevents a concurrent edit from cancelling this snapshot.
                                                    let mut engine = engine_arc.lock_owned().await;
                                                    tokio::task::spawn_blocking(move || {
                                                        if let Err(e) = engine.activate_session(session_id) {
                                                            tracing::warn!(error = %e, session = session_id, "session view activation failed");
                                                        }
                                                        engine.find_all_refs(&fp_clone, line + 1, col + 1).unwrap_or_else(|e| {
                                                            tracing::warn!(error = %e, session = session_id, "query failed");
                                                            Vec::new()
                                                        })
                                                    })
                                                    .await
                                                    .unwrap_or_default()
                                                };

                                                let duration = query_start.elapsed();
                                                let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                                let ms = duration.as_secs_f64() * 1000.0;
                                                let count = refs.len();

                                                if ms > 200.0 {
                                                    SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                    tracing::warn!(
                                                        req = req_num,
                                                        session = session_id,
                                                        method = "textDocument/references",
                                                        duration_ms = format!("{:.2}ms", ms),
                                                        references = count,
                                                        in_flight = remaining,
                                                        "⚠️ [LSP SLOW >200ms]"
                                                    );
                                                } else {
                                                    tracing::info!(
                                                        req = req_num,
                                                        session = session_id,
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
                                                let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                            });
                                            continue;
                                        }
                                    }
                                    Some("textDocument/documentSymbol") => {
                                        if let Some(params) = val.get("params") {
                                             let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("").to_string();
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

                                             let engine_arc = Arc::clone(engine_lock);

                                             let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                             let fp_clone = file_path.clone();
                                             let out_tx_task = out_tx.clone();
                                             let translator_task = translator.clone();
                                             let session_id = view.session_id;

                                             tokio::task::spawn(async move {
                                                 let syms = {
                                                     // Hold the engine for the whole query: activating the session view and
                                                     // running the query under one lock keeps other sessions' buffers out and
                                                     // prevents a concurrent edit from cancelling this snapshot.
                                                     let mut engine = engine_arc.lock_owned().await;
                                                     tokio::task::spawn_blocking(move || {
                                                         if let Err(e) = engine.activate_session(session_id) {
                                                             tracing::warn!(error = %e, session = session_id, "session view activation failed");
                                                         }
                                                         engine.document_symbols(&fp_clone).unwrap_or_else(|e| {
                                                             tracing::warn!(error = %e, session = session_id, "query failed");
                                                             Vec::new()
                                                         })
                                                     })
                                                     .await
                                                     .unwrap_or_default()
                                                 };

                                                 let duration = query_start.elapsed();
                                                 let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                                 let ms = duration.as_secs_f64() * 1000.0;
                                                 let count = syms.len();

                                                 if ms > 200.0 {
                                                     SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                     tracing::warn!(
                                                         req = req_num,
                                                         session = session_id,
                                                         method = "textDocument/documentSymbol",
                                                         duration_ms = format!("{:.2}ms", ms),
                                                         symbols = count,
                                                         in_flight = remaining,
                                                         "⚠️ [LSP SLOW >200ms]"
                                                     );
                                                 } else {
                                                     tracing::info!(
                                                         req = req_num,
                                                         session = session_id,
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
                                                 let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                 let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                             });
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
                                                    if let Err(e) = engine.set_session_overlay(view.session_id, &file_path, Some(text.to_string())) {
                                                        tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                                    }
                                                }
                                                let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                                tracing::info!(
                                                    session = view.session_id,
                                                    file = %file_path.display(),
                                                    bytes = text_len,
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    "📝 [OVERLAY] didOpen recorded as session buffer in Salsa DB"
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
                                                    if let Err(e) = engine.set_session_overlay(view.session_id, &file_path, Some(text.to_string())) {
                                                        tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                                    }
                                                }
                                                let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                                tracing::info!(
                                                    session = view.session_id,
                                                    file = %file_path.display(),
                                                    bytes = text_len,
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    "📝 [OVERLAY] didChange recorded as session buffer in Salsa DB"
                                                );
                                            }
                                        }
                                    }
                                    Some("textDocument/didClose") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));
                                            let mut engine = engine_lock.lock().await;
                                            if let Err(e) = engine.clear_session_overlay(view.session_id, &file_path) {
                                                tracing::warn!(error = %e, file = %file_path.display(), "session overlay close failed");
                                            }
                                        }
                                        continue;
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
                                        let out_tx_task = out_tx.clone();
                                        let go_clone = Arc::clone(go);
                                        let translator_task = translator.clone();
                                        let session_id = view.session_id;
                                        let method_str = m.to_string();
                                        let req_id = id.clone();

                                        tokio::task::spawn(async move {
                                            let resp_res = go_clone.send_request(&method_str, params).await;
                                            let duration = start.elapsed();
                                            let duration_ms = duration.as_secs_f64() * 1000.0;
                                            let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

                                            if duration_ms > 200.0 {
                                                SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                tracing::warn!(
                                                    req = req_id_log,
                                                    session = session_id,
                                                    method = %method_str,
                                                    duration_ms = %format!("{:.2}ms", duration_ms),
                                                    in_flight = remaining,
                                                    "⚠️ [LSP SLOW >200ms] GoEngine query exceeded threshold"
                                                );
                                            } else {
                                                tracing::info!(
                                                    req = req_id_log,
                                                    session = session_id,
                                                    method = %method_str,
                                                    duration_ms = %format!("{:.2}ms", duration_ms),
                                                    in_flight = remaining,
                                                    "✅ [LSP DONE] GoEngine query complete"
                                                );
                                            }

                                            match resp_res {
                                                Ok(mut resp) => {
                                                    if let Some(ref r_id) = req_id {
                                                        resp["id"] = r_id.clone();
                                                    }
                                                    let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                    let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                                }
                                                Err(err) => {
                                                    let err_resp = serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": req_id,
                                                        "error": { "code": -32603, "message": err.to_string() }
                                                    });
                                                    let _ = out_tx_task.send(WireMessage::LspPayload(err_resp.to_string())).await;
                                                }
                                            }
                                        });
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
                                    let out_tx_task = out_tx.clone();
                                    let generic_eng_clone = Arc::clone(generic_eng);
                                    let translator_task = translator.clone();
                                    let session_id = view.session_id;
                                    let method_str = m.to_string();
                                    let r_id = req_id.clone();

                                    tokio::task::spawn(async move {
                                        let resp_res = generic_eng_clone.send_request(&method_str, params).await;
                                        let duration = start.elapsed();
                                        let duration_ms = duration.as_secs_f64() * 1000.0;
                                        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

                                        if duration_ms > 200.0 {
                                            SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                            tracing::warn!(
                                                req = req_id_log,
                                                session = session_id,
                                                method = %method_str,
                                                duration_ms = %format!("{:.2}ms", duration_ms),
                                                in_flight = remaining,
                                                "⚠️ [LSP SLOW >200ms] GenericLspEngine query exceeded threshold"
                                            );
                                        } else {
                                            tracing::info!(
                                                req = req_id_log,
                                                session = session_id,
                                                method = %method_str,
                                                duration_ms = %format!("{:.2}ms", duration_ms),
                                                in_flight = remaining,
                                                "✅ [LSP DONE] GenericLspEngine query complete"
                                            );
                                        }

                                        match resp_res {
                                            Ok(mut resp) => {
                                                resp["id"] = r_id;
                                                let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                            }
                                            Err(err) => {
                                                let err_resp = serde_json::json!({
                                                    "jsonrpc": "2.0",
                                                    "id": r_id,
                                                    "error": { "code": -32603, "message": err.to_string() }
                                                });
                                                let _ = out_tx_task.send(WireMessage::LspPayload(err_resp.to_string())).await;
                                            }
                                        }
                                    });
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
                                let _ = out_tx.send(WireMessage::LspPayload(empty_resp.to_string())).await;
                                continue;
                            }
                        }

                        if let Some(backend) = &view.workspace.backend {
                            let _ = backend.send_lsp(&server_lsp).await.inspect_err(|e| {
                                tracing::error!(error = %e, "Failed to forward LSP to backend worker");
                            });
                        }
                    }
                    Some(Ok(WireMessage::SyncRequest(req))) => {
                        let start = Instant::now();
                        let mut files_updated = 0;
                        let mut files_deleted = 0;
                        let mut bytes_transferred = 0;

                        for delta in &req.files {
                            let target_path = view.workspace.root.join(&delta.relative_path);
                            match &delta.content {
                                Some(content_bytes) => {
                                    if let Some(parent) = target_path.parent() {
                                        let _ = tokio::fs::create_dir_all(parent).await;
                                    }
                                    bytes_transferred += content_bytes.len();
                                    if tokio::fs::write(&target_path, content_bytes).await.is_ok() {
                                        files_updated += 1;
                                    }
                                    // The workspace is this worktree's own: synced files are its
                                    // new base, visible to every session except one that still
                                    // holds an unsaved buffer for the same path.
                                    if let (Some(engine_lock), Ok(text)) =
                                        (&view.workspace.rust_engine, std::str::from_utf8(content_bytes))
                                    {
                                        let mut engine = engine_lock.lock().await;
                                        if let Err(e) = engine.update_base(&target_path, Some(text.to_string())) {
                                            tracing::warn!(error = %e, file = %target_path.display(), "base update failed");
                                        }
                                    }
                                }
                                None => {
                                    if target_path.exists()
                                        && tokio::fs::remove_file(&target_path).await.is_ok()
                                    {
                                        files_deleted += 1;
                                    }
                                    if let Some(engine_lock) = &view.workspace.rust_engine {
                                        let mut engine = engine_lock.lock().await;
                                        if let Err(e) = engine.update_base(&target_path, None) {
                                            tracing::warn!(error = %e, file = %target_path.display(), "base removal failed");
                                        }
                                    }
                                }
                            }
                        }

                        if req.clean_others
                            && let Some(engine_lock) = &view.workspace.rust_engine
                        {
                            // The request is the session's complete dirty set: any other
                            // overlay this session still holds is stale (reverted or committed).
                            let keep: Vec<PathBuf> = req
                                .files
                                .iter()
                                .map(|delta| view.workspace.root.join(&delta.relative_path))
                                .collect();
                            let mut engine = engine_lock.lock().await;
                            match engine.retain_session_overlays(view.session_id, &keep) {
                                Ok(dropped) if dropped > 0 => tracing::info!(
                                    session = view.session_id,
                                    dropped,
                                    "🧹 [OVERLAY] dropped stale session buffers after full dirty sync"
                                ),
                                Ok(_) => {}
                                Err(e) => tracing::warn!(error = %e, session = view.session_id, "failed to drop stale session buffers"),
                            }
                        }

                        let duration_ms = start.elapsed().as_millis() as u64;
                        let _ = out_tx
                            .send(WireMessage::SyncResponse(SyncResponse {
                                files_updated,
                                files_deleted,
                                bytes_transferred,
                                duration_ms,
                                server_workspace_root: view.workspace.root.to_string_lossy().to_string(),
                            }))
                            .await;
                    }
                    Some(Ok(WireMessage::Disconnect { reason })) => {
                        tracing::info!(reason, "Client terminated session");
                        break;
                    }
                    Some(Ok(WireMessage::StatusRequest)) => {
                        let _ = out_tx
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
                            .await;
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
                        if out_tx.send(WireMessage::LspPayload(client_lsp)).await.is_err() {
                            tracing::error!("Failed to send LSP message to client channel");
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
    drop(out_tx);
    let _ = writer_handle.await;
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

    #[tokio::test]
    async fn test_apply_sync_create_and_delete() {
        use prod_code_protocol::FileDelta;

        let storage_temp = tempfile::tempdir().unwrap();
        let client_root = "/Users/testuser/Projects/my-app";

        let req = SyncRequest {
            client_workspace_root: client_root.to_string(),
            files: vec![
                FileDelta {
                    relative_path: "src/lib.rs".to_string(),
                    content: Some(b"pub fn add(a: i32, b: i32) -> i32 { a + b }".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: "README.md".to_string(),
                    content: Some(b"# My App".to_vec()),
                    is_executable: false,
                },
            ],
            clean_others: false,
            base_workspace_name: None,
        };

        let resp = apply_sync(storage_temp.path(), req).await;
        assert_eq!(resp.files_updated, 2);
        assert_eq!(resp.files_deleted, 0);

        let app_dir = storage_temp.path().join("my-app");
        assert!(app_dir.join("src/lib.rs").exists());
        assert!(app_dir.join("README.md").exists());
        let content = std::fs::read_to_string(app_dir.join("src/lib.rs")).unwrap();
        assert!(content.contains("pub fn add"));

        // Now test deleting README.md
        let del_req = SyncRequest {
            client_workspace_root: client_root.to_string(),
            files: vec![FileDelta {
                relative_path: "README.md".to_string(),
                content: None,
                is_executable: false,
            }],
            clean_others: false,
            base_workspace_name: None,
        };

        let del_resp = apply_sync(storage_temp.path(), del_req).await;
        assert_eq!(del_resp.files_updated, 0);
        assert_eq!(del_resp.files_deleted, 1);
        assert!(!app_dir.join("README.md").exists());
        assert!(app_dir.join("src/lib.rs").exists());
    }
}
