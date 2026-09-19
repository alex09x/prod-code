//! prod-code gateway daemon: multi-tenant server for remote code intelligence over 10 GbE LAN.

pub mod backend;
pub mod workspace;

use anyhow::Result;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeResponse, PathTranslator, ProdCodeCodec, StatusResponse, WireMessage, PROTOCOL_VERSION,
};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;
use workspace::{SessionView, WorkspaceManager};

#[derive(Parser, Debug)]
#[command(name = "prod-code-server", author, version, about = "Remote Code Intelligence Gateway")]
pub struct ServerCli {
    /// Bind address (IP:port). Defaults to 0.0.0.0:9400.
    #[arg(short, long, env = "PROD_CODE_BIND", default_value = "0.0.0.0:9400")]
    pub bind: SocketAddr,

    /// Workspace root storage directory on server.
    #[arg(short, long, env = "PROD_CODE_STORAGE", default_value = "/srv/prod-code/workspaces")]
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
        }
    }
}

pub fn detect_engine(root: &Path) -> &'static str {
    if root.join("Cargo.toml").exists() {
        "rust"
    } else if root.join("go.mod").exists() {
        "go"
    } else if root.join("pyproject.toml").exists() || root.join("requirements.txt").exists() {
        "python"
    } else if root.join("package.json").exists() {
        "typescript"
    } else {
        "generic"
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

                let engine = detect_engine(&server_workspace);
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
                let session_res =
                    run_session_loop(&mut framed, &translator, &session_view).await;

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
    let mut backend_rx = view.workspace.backend.as_ref().map(|b| b.subscribe());

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

                        if let Some(ref backend) = view.workspace.backend {
                            if let Err(e) = backend.send_lsp(&server_lsp).await {
                                tracing::error!(error = %e, "Failed to forward LSP to backend worker");
                            }
                        } else {
                            // Fallback route if no backend worker is configured
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&server_lsp) {
                                if val.get("method").and_then(|m| m.as_str()) == Some("initialize") {
                                    let id = val.get("id").cloned().unwrap_or(serde_json::json!(1));
                                    let init_resp = serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": {
                                            "capabilities": {
                                                "textDocumentSync": 1,
                                                "hoverProvider": true,
                                                "definitionProvider": true,
                                                "referencesProvider": true,
                                                "documentSymbolProvider": true,
                                                "workspaceSymbolProvider": true
                                            },
                                            "serverInfo": {
                                                "name": "prod-code-rci",
                                                "version": "0.1.0"
                                            }
                                        }
                                    });
                                    let client_resp = translator.translate_lsp_to_client(&init_resp.to_string());
                                    framed.send(WireMessage::LspPayload(client_resp)).await?;
                                } else if val.get("method").and_then(|m| m.as_str()) == Some("shutdown") {
                                    let id = val.get("id").cloned().unwrap_or(serde_json::json!(1));
                                    let shutdown_resp = serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": id,
                                        "result": null
                                    });
                                    framed
                                        .send(WireMessage::LspPayload(shutdown_resp.to_string()))
                                        .await?;
                                }
                            }
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
                    futures_util::future::pending().await
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
    tracing_subscriber::fmt::init();
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
        assert!(status.detected_engines.contains(&"rust (ra_ap_ide)".to_string()));
    }

    #[test]
    fn test_engine_detection() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(detect_engine(temp.path()), "generic");

        std::fs::write(temp.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_engine(temp.path()), "rust");

        let go_temp = tempfile::tempdir().unwrap();
        std::fs::write(go_temp.path().join("go.mod"), "").unwrap();
        assert_eq!(detect_engine(go_temp.path()), "go");

        let py_temp = tempfile::tempdir().unwrap();
        std::fs::write(py_temp.path().join("pyproject.toml"), "").unwrap();
        assert_eq!(detect_engine(py_temp.path()), "python");

        let ts_temp = tempfile::tempdir().unwrap();
        std::fs::write(ts_temp.path().join("package.json"), "").unwrap();
        assert_eq!(detect_engine(ts_temp.path()), "typescript");
    }
}
