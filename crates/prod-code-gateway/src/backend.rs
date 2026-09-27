//! Managed language server backend workers (e.g. rust-analyzer on a Linux node) supervised by prod-code gateway.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, Notify, RwLock, broadcast};

/// Managed backend worker running a language server process on the host.
pub struct BackendWorker {
    pub engine: String,
    pub workspace_root: String,
    stdin: Arc<Mutex<ChildStdin>>,
    broadcast_tx: broadcast::Sender<String>,
    pub capabilities: Arc<RwLock<Option<serde_json::Value>>>,
    pub open_files: Arc<RwLock<HashSet<String>>>,
    is_alive: Arc<AtomicBool>,
    closed: Arc<Notify>,
    _child: Arc<Mutex<Child>>,
}

impl BackendWorker {
    /// Spawn a language server worker for the specified workspace and initialize it.
    pub async fn spawn(workspace_root: &Path, engine: &str) -> Result<Self> {
        let binary = match engine {
            "rust" => "rust-analyzer",
            "go" => "gopls",
            other => anyhow::bail!("no managed backend language server for engine `{other}`"),
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
        let is_alive = Arc::new(AtomicBool::new(true));
        let reader_alive = Arc::clone(&is_alive);
        let closed = Arc::new(Notify::new());
        let reader_closed = Arc::clone(&closed);
        let child = Arc::new(Mutex::new(child));
        let reader_child = Arc::downgrade(&child);

        // Background reader loop: reads Content-Length frames from language server stdout
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let json = match prod_code_protocol::transport::read_lsp_frame(&mut reader).await {
                    Ok(Some(json)) => json,
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(%error, "Invalid backend LSP frame");
                        break;
                    }
                };
                // Auto-respond to server-initiated requests
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) {
                    let id = val.get("id");
                    let method = val.get("method").and_then(|m| m.as_str());
                    match (id, method) {
                        (
                            Some(id),
                            Some("window/workDoneProgress/create" | "client/registerCapability"),
                        ) => {
                            let auto_resp = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": null
                            })
                            .to_string();
                            let header = format!("Content-Length: {}\r\n\r\n", auto_resp.len());
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
                            let header = format!("Content-Length: {}\r\n\r\n", auto_resp.len());
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
            reader_alive.store(false, Ordering::Release);
            reader_closed.notify_one();
            if let Some(child) = reader_child.upgrade() {
                let _ = child.lock().await.start_kill();
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
            is_alive,
            closed,
            _child: child,
        };

        // Perform backend initialization handshake so the backend is warm and ready
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            worker.initialize_backend(workspace_root),
        )
        .await
        .context("Timeout initializing backend language server")?
        .context("Failed to initialize backend language server")?;

        Ok(worker)
    }

    /// Perform the one-time LSP initialize handshake with the backend worker.
    async fn initialize_backend(&self, workspace_root: &Path) -> Result<()> {
        let ws_uri = prod_code_protocol::path::file_uri(workspace_root);
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
                "rootUri": ws_uri,
                "workspaceFolders": [
                    {
                        "name": ws_name,
                        "uri": ws_uri
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

        // Server requests have their own IDs and may collide with our initialize request.
        // Only a response with the complete capabilities object establishes a usable server.
        loop {
            anyhow::ensure!(self.is_alive(), "backend exited during initialization");
            let message = tokio::select! {
                _ = self.closed.notified() => anyhow::bail!("backend exited during initialization"),
                message = rx.recv() => message.context("backend initialization response stream closed")?,
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&message) else {
                continue;
            };
            if value.get("method").is_some()
                || value.get("id").and_then(serde_json::Value::as_u64) != Some(1)
            {
                continue;
            }
            if let Some(error) = value.get("error") {
                anyhow::bail!("backend refused initialization: {error}");
            }
            let capabilities = value
                .pointer("/result/capabilities")
                .filter(|capabilities| capabilities.is_object())
                .context("backend initialize response has no capabilities object")?;
            *self.capabilities.write().await = Some(capabilities.clone());
            break;
        }

        // Send initialized notification
        let initialized = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        });
        self.send_lsp(&initialized.to_string()).await?;

        tracing::info!(workspace = %workspace_root.display(), "Backend language server initialized and warm");
        Ok(())
    }

    /// False after the managed language server's output has ended.
    pub fn is_alive(&self) -> bool {
        self.is_alive.load(Ordering::Acquire)
    }

    /// Subscribe to raw LSP frames produced by this backend worker.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.broadcast_tx.subscribe()
    }

    /// Send an LSP JSON-RPC message into the backend language server's stdin.
    pub async fn send_lsp(&self, json_payload: &str) -> Result<()> {
        anyhow::ensure!(
            self.is_alive(),
            "{} backend process has exited",
            self.engine
        );
        let mut stdin = self.stdin.lock().await;
        anyhow::ensure!(
            self.is_alive(),
            "{} backend process has exited",
            self.engine
        );
        let header = format!("Content-Length: {}\r\n\r\n", json_payload.len());
        stdin.write_all(header.as_bytes()).await?;
        stdin.write_all(json_payload.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }
}
