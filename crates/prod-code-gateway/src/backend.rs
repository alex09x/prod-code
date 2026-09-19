//! Managed language server backend workers (e.g. rust-analyzer on booster) supervised by prod-code gateway.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, RwLock, broadcast};

/// Managed backend worker running a language server process on the host.
pub struct BackendWorker {
    pub engine: String,
    pub workspace_root: String,
    stdin: Arc<Mutex<ChildStdin>>,
    broadcast_tx: broadcast::Sender<String>,
    pub capabilities: Arc<RwLock<Option<serde_json::Value>>>,
    pub open_files: Arc<RwLock<HashSet<String>>>,
    _child: Arc<Mutex<Child>>,
}

impl BackendWorker {
    /// Spawn a language server worker for the specified workspace and initialize it.
    pub async fn spawn(workspace_root: &Path, engine: &str) -> Result<Self> {
        let binary = match engine {
            "rust" => "rust-analyzer",
            "go" => "gopls",
            _ => "rust-analyzer",
        };

        tracing::info!(
            engine,
            binary,
            ?workspace_root,
            "Spawning backend language server"
        );

        let mut cmd = Command::new(binary);
        cmd.kill_on_drop(true);
        cmd.current_dir(workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn {binary} at {:?}", workspace_root))?;

        let stdin = child
            .stdin
            .take()
            .context("Failed to open stdin for backend language server")?;
        let stdout = child
            .stdout
            .take()
            .context("Failed to open stdout for backend language server")?;

        let (tx, _rx) = broadcast::channel::<String>(2048);
        let tx_clone = tx.clone();
        let stdin_arc = Arc::new(Mutex::new(stdin));
        let stdin_writer = stdin_arc.clone();

        // Background reader loop: reads Content-Length frames from language server stdout
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();

            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        if line.starts_with("Content-Length:") {
                            let len_str = line.trim_start_matches("Content-Length:").trim();
                            if let Ok(len) = len_str.parse::<usize>() {
                                // Read empty separator line \r\n
                                line.clear();
                                let _ = reader.read_line(&mut line).await;

                                let mut buf = vec![0u8; len];
                                if reader.read_exact(&mut buf).await.is_err() {
                                    continue;
                                }
                                let Ok(json) = String::from_utf8(buf) else {
                                    continue;
                                };

                                // Auto-respond to server-initiated requests
                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) {
                                    let id = val.get("id");
                                    let method = val.get("method").and_then(|m| m.as_str());
                                    match (id, method) {
                                        (
                                            Some(id),
                                            Some(
                                                "window/workDoneProgress/create"
                                                | "client/registerCapability",
                                            ),
                                        ) => {
                                            let auto_resp = serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": id,
                                                "result": null
                                            })
                                            .to_string();
                                            let header = format!(
                                                "Content-Length: {}\r\n\r\n",
                                                auto_resp.len()
                                            );
                                            let mut sin = stdin_writer.lock().await;
                                            let _ = sin.write_all(header.as_bytes()).await;
                                            let _ = sin.write_all(auto_resp.as_bytes()).await;
                                            let _ = sin.flush().await;
                                        }
                                        (Some(id), Some("workspace/configuration")) => {
                                            let auto_resp = serde_json::json!({
                                                "jsonrpc": "2.0",
                                                "id": id,
                                                "result": [{}]
                                            })
                                            .to_string();
                                            let header = format!(
                                                "Content-Length: {}\r\n\r\n",
                                                auto_resp.len()
                                            );
                                            let mut sin = stdin_writer.lock().await;
                                            let _ = sin.write_all(header.as_bytes()).await;
                                            let _ = sin.write_all(auto_resp.as_bytes()).await;
                                            let _ = sin.flush().await;
                                        }
                                        _ => {}
                                    }
                                }

                                // Broadcast frame to all connected sessions
                                let _ = tx_clone.send(json);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            tracing::info!("Backend worker reader loop terminated");
        });

        let worker = Self {
            engine: engine.to_string(),
            workspace_root: workspace_root.to_string_lossy().to_string(),
            stdin: stdin_arc,
            broadcast_tx: tx,
            capabilities: Arc::new(RwLock::new(None)),
            open_files: Arc::new(RwLock::new(HashSet::new())),
            _child: Arc::new(Mutex::new(child)),
        };

        // Perform backend initialization handshake so the backend is warm and ready
        if let Err(e) = worker.initialize_backend(workspace_root).await {
            tracing::warn!(error = %e, "Initial backend handshake error (will proceed anyway)");
        }

        Ok(worker)
    }

    /// Perform the one-time LSP initialize handshake with the backend worker.
    async fn initialize_backend(&self, workspace_root: &Path) -> Result<()> {
        let ws_str = workspace_root.to_string_lossy().to_string();
        let ws_name = workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace");

        let init_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": format!("file://{}", ws_str),
                "workspaceFolders": [
                    {
                        "name": ws_name,
                        "uri": format!("file://{}", ws_str)
                    }
                ],
                "capabilities": {
                    "workspace": {
                        "workspaceFolders": true,
                        "configuration": true
                    },
                    "textDocument": {
                        "hover": {
                            "contentFormat": ["markdown", "plaintext"]
                        },
                        "definition": {
                            "linkSupport": true
                        },
                        "documentSymbol": {
                            "hierarchicalDocumentSymbolSupport": true
                        },
                        "references": {}
                    }
                }
            }
        });

        let mut rx = self.subscribe();
        self.send_lsp(&init_req.to_string()).await?;

        // Wait for initialize response (id = 1)
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(json)) => {
                    let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) else {
                        continue;
                    };
                    if val.get("id").and_then(|id| id.as_i64()) == Some(1) {
                        if let Some(caps) = val.get("result").and_then(|r| r.get("capabilities")) {
                            *self.capabilities.write().await = Some(caps.clone());
                        }
                        break;
                    }
                }
                Ok(Err(_)) => {}
                Err(_) => {
                    tracing::warn!("Timeout waiting for backend initialize response, continuing");
                    break;
                }
            }
        }

        // Send initialized notification
        let initialized = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        });
        self.send_lsp(&initialized.to_string()).await?;

        tracing::info!(workspace = %ws_str, "Backend language server initialized and warm");
        Ok(())
    }

    /// Subscribe to raw LSP frames produced by this backend worker.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.broadcast_tx.subscribe()
    }

    /// Send an LSP JSON-RPC message into the backend language server's stdin.
    pub async fn send_lsp(&self, json_payload: &str) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        let header = format!("Content-Length: {}\r\n\r\n", json_payload.len());
        stdin.write_all(header.as_bytes()).await?;
        stdin.write_all(json_payload.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }
}
