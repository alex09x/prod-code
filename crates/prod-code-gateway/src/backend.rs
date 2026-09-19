//! Managed language server backend workers (e.g. rust-analyzer on booster) supervised by prod-code gateway.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, Mutex};

/// Managed backend worker running a language server process on the host.
pub struct BackendWorker {
    pub engine: String,
    pub workspace_root: String,
    stdin: Arc<Mutex<ChildStdin>>,
    broadcast_tx: broadcast::Sender<String>,
    _child: Arc<Mutex<Child>>,
}

impl BackendWorker {
    /// Spawn a language server worker for the specified workspace.
    pub async fn spawn(workspace_root: &Path, engine: &str) -> Result<Self> {
        let binary = match engine {
            "rust" => "rust-analyzer",
            "go" => "gopls",
            _ => "rust-analyzer",
        };

        tracing::info!(engine, binary, ?workspace_root, "Spawning backend language server");

        let mut cmd = Command::new(binary);
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
                                if reader.read_exact(&mut buf).await.is_ok() {
                                    if let Ok(json) = String::from_utf8(buf) {
                                        // Auto-respond to server-initiated requests
                                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) {
                                            if let (Some(id), Some(method)) = (val.get("id"), val.get("method").and_then(|m| m.as_str())) {
                                                match method {
                                                    "window/workDoneProgress/create" | "client/registerCapability" => {
                                                        let auto_resp = serde_json::json!({
                                                            "jsonrpc": "2.0",
                                                            "id": id,
                                                            "result": null
                                                        }).to_string();
                                                        let header = format!("Content-Length: {}\r\n\r\n", auto_resp.len());
                                                        let mut sin = stdin_writer.lock().await;
                                                        let _ = sin.write_all(header.as_bytes()).await;
                                                        let _ = sin.write_all(auto_resp.as_bytes()).await;
                                                        let _ = sin.flush().await;
                                                    }
                                                    "workspace/configuration" => {
                                                        let auto_resp = serde_json::json!({
                                                            "jsonrpc": "2.0",
                                                            "id": id,
                                                            "result": [{}]
                                                        }).to_string();
                                                        let header = format!("Content-Length: {}\r\n\r\n", auto_resp.len());
                                                        let mut sin = stdin_writer.lock().await;
                                                        let _ = sin.write_all(header.as_bytes()).await;
                                                        let _ = sin.write_all(auto_resp.as_bytes()).await;
                                                        let _ = sin.flush().await;
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }

                                        // Broadcast frame to all connected sessions
                                        let _ = tx_clone.send(json);
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            tracing::info!("Backend worker reader loop terminated");
        });

        Ok(Self {
            engine: engine.to_string(),
            workspace_root: workspace_root.to_string_lossy().to_string(),
            stdin: stdin_arc,
            broadcast_tx: tx,
            _child: Arc::new(Mutex::new(child)),
        })
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
