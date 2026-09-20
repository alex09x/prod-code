//! Managed Go language intelligence engine powered by supervised `gopls`.
//!
//! Provides multi-worktree Go code intelligence with shared GOCACHE and GOMODCACHE
//! for high-performance symbol resolution and compilation reuse.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, RwLock, broadcast, oneshot};

/// Configuration for the managed Go engine.
#[derive(Debug, Clone, Default)]
pub struct GoConfig {
    /// Optional explicit path to the `gopls` binary.
    pub gopls_path: Option<PathBuf>,
    /// Shared cache directory for `GOCACHE` and `GOMODCACHE`.
    pub shared_cache_dir: Option<PathBuf>,
    /// Additional environment variables for the Go toolchain.
    pub extra_env: HashMap<String, String>,
    /// Build flags / tags (e.g. `["-tags=integration"]`).
    pub build_flags: Vec<String>,
}

/// Discovers the `gopls` executable on the host system.
pub fn find_gopls_binary(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit.filter(|p| p.exists()) {
        return Some(path.to_path_buf());
    }

    // Check $PATH via which
    if let Ok(path) = which_gopls() {
        return Some(path);
    }

    // Standard Go installation locations
    let mut candidates = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(&home).join("go/bin/gopls"));
        candidates.push(PathBuf::from(&home).join(".local/bin/gopls"));
    }
    if let Ok(gopath) = std::env::var("GOPATH") {
        candidates.push(PathBuf::from(gopath).join("bin/gopls"));
    }
    candidates.push(PathBuf::from("/usr/local/bin/gopls"));
    candidates.push(PathBuf::from("/snap/bin/gopls"));
    candidates.push(PathBuf::from("/opt/homebrew/bin/gopls"));

    candidates.into_iter().find(|p| p.exists())
}

fn which_gopls() -> Result<PathBuf> {
    let output = std::process::Command::new("which")
        .arg("gopls")
        .output()
        .context("which command failed")?;
    if output.status.success() {
        let path_str = String::from_utf8(output.stdout)?.trim().to_string();
        if !path_str.is_empty() {
            return Ok(PathBuf::from(path_str));
        }
    }
    anyhow::bail!("gopls not found in PATH")
}

/// Supervised Go engine managing an active `gopls` worker instance.
pub struct GoEngine {
    workspace_root: PathBuf,
    stdin: Arc<Mutex<ChildStdin>>,
    next_req_id: AtomicU64,
    pending_requests: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    pub capabilities: Arc<RwLock<Option<serde_json::Value>>>,
    broadcast_tx: broadcast::Sender<String>,
    _child: Arc<Mutex<Child>>,
}

impl GoEngine {
    /// Launch a new managed Go engine for the given workspace root.
    pub async fn load(workspace_root: &Path, config: GoConfig) -> Result<Self> {
        let gopls_bin = find_gopls_binary(config.gopls_path.as_deref())
            .ok_or_else(|| anyhow::anyhow!("gopls executable not found on host"))?;

        // Determine shared NVMe cache directories
        let cache_base = config.shared_cache_dir.unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".cache/prod-code/go"))
                .unwrap_or_else(|_| std::env::temp_dir().join("prod-code-go-cache"))
        });
        let gocache = cache_base.join("gocache");
        let gomodcache = cache_base.join("modcache");

        tokio::fs::create_dir_all(&gocache)
            .await
            .context("Failed to create GOCACHE directory")?;
        tokio::fs::create_dir_all(&gomodcache)
            .await
            .context("Failed to create GOMODCACHE directory")?;

        tracing::info!(
            workspace = ?workspace_root,
            gopls = ?gopls_bin,
            ?gocache,
            ?gomodcache,
            "Spawning supervised gopls worker"
        );

        let mut cmd = Command::new(&gopls_bin);
        cmd.kill_on_drop(true);
        cmd.current_dir(workspace_root)
            .env("GOCACHE", &gocache)
            .env("GOMODCACHE", &gomodcache)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        for (k, v) in &config.extra_env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn gopls binary: {:?}", gopls_bin))?;

        let stdin = child
            .stdin
            .take()
            .context("Failed to capture gopls child stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("Failed to capture gopls child stdout")?;

        let (bcast_tx, _) = broadcast::channel(1024);
        let bcast_tx_clone = bcast_tx.clone();

        let pending_requests: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending_requests.clone();

        let stdin_arc = Arc::new(Mutex::new(stdin));
        let stdin_writer = stdin_arc.clone();

        // Background reader loop: decodes LSP frames and routes responses to oneshot channels
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut header_line = String::new();

            loop {
                header_line.clear();
                match reader.read_line(&mut header_line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        if header_line.starts_with("Content-Length:") {
                            let len_str = header_line.trim_start_matches("Content-Length:").trim();
                            if let Ok(len) = len_str.parse::<usize>() {
                                // Read the blank line separator (\r\n)
                                header_line.clear();
                                let _ = reader.read_line(&mut header_line).await;

                                let mut body = vec![0u8; len];
                                if reader.read_exact(&mut body).await.is_err() {
                                    continue;
                                }
                                let Ok(json_str) = String::from_utf8(body) else {
                                    continue;
                                };

                                if let Ok(val) =
                                    serde_json::from_str::<serde_json::Value>(&json_str)
                                {
                                    // 1. Check if this is a response to our pending request
                                    if let Some(id_val) = val.get("id") {
                                        if let Some(id) = id_val.as_u64() {
                                            let mut pending = pending_clone.lock().await;
                                            if let Some(tx) = pending.remove(&id) {
                                                let _ = tx.send(val.clone());
                                                continue;
                                            }
                                        }

                                        // Server-initiated request requiring auto-reply
                                        let method = val.get("method").and_then(|m| m.as_str());
                                        if let Some(m) = method {
                                            match m {
                                                "window/workDoneProgress/create"
                                                | "client/registerCapability" => {
                                                    let auto_resp = serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": id_val,
                                                        "result": null
                                                    });
                                                    let _ = Self::write_frame_raw(
                                                        &stdin_writer,
                                                        &auto_resp,
                                                    )
                                                    .await;
                                                }
                                                "workspace/configuration" => {
                                                    let auto_resp = serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": id_val,
                                                        "result": [{}]
                                                    });
                                                    let _ = Self::write_frame_raw(
                                                        &stdin_writer,
                                                        &auto_resp,
                                                    )
                                                    .await;
                                                }
                                                _ => {}
                                            }
                                        }
                                    }

                                    // Broadcast notification to listeners
                                    let _ = bcast_tx_clone.send(json_str);
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            tracing::info!("gopls background reader loop stopped");
        });

        let engine = Self {
            workspace_root: workspace_root.to_path_buf(),
            stdin: stdin_arc,
            next_req_id: AtomicU64::new(1),
            pending_requests,
            capabilities: Arc::new(RwLock::new(None)),
            broadcast_tx: bcast_tx,
            _child: Arc::new(Mutex::new(child)),
        };

        // Initialize gopls with workspace root
        engine.initialize().await?;

        Ok(engine)
    }

    /// Helper to write an LSP Content-Length frame directly to child stdin.
    async fn write_frame_raw(
        writer: &Arc<Mutex<ChildStdin>>,
        val: &serde_json::Value,
    ) -> Result<()> {
        let body = val.to_string();
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut sin = writer.lock().await;
        sin.write_all(frame.as_bytes()).await?;
        sin.flush().await?;
        Ok(())
    }

    /// Perform the one-time LSP initialize handshake with `gopls`.
    pub async fn initialize(&self) -> Result<serde_json::Value> {
        let ws_str = self.workspace_root.to_string_lossy().to_string();
        let ws_name = self
            .workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("go-workspace");

        let init_params = serde_json::json!({
            "processId": std::process::id(),
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
        });

        let resp = self.send_request("initialize", init_params).await?;

        if let Some(caps) = resp.get("result").and_then(|r| r.get("capabilities")) {
            let mut guard = self.capabilities.write().await;
            *guard = Some(caps.clone());
        }

        // Send initialized notification as required by LSP spec
        self.send_notification("initialized", serde_json::json!({}))
            .await?;

        Ok(resp)
    }

    /// Send a typed JSON-RPC request and wait for the response.
    pub async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let req_id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(req_id, tx);
        }

        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params
        });

        Self::write_frame_raw(&self.stdin, &payload).await?;

        // Await response with timeout
        match tokio::time::timeout(tokio::time::Duration::from_secs(30), rx).await {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(_)) => anyhow::bail!("gopls request channel closed unexpectedly"),
            Err(_) => {
                let mut pending = self.pending_requests.lock().await;
                pending.remove(&req_id);
                anyhow::bail!("Timeout waiting for gopls response to method '{method}'");
            }
        }
    }

    /// Send an asynchronous JSON-RPC notification to `gopls`.
    pub async fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        Self::write_frame_raw(&self.stdin, &payload).await
    }

    /// Notify `gopls` that a document was opened in an editor or worktree.
    pub async fn did_open(&self, file_uri: &str, text: &str) -> Result<()> {
        self.send_notification(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": file_uri,
                    "languageId": "go",
                    "version": 1,
                    "text": text
                }
            }),
        )
        .await
    }

    /// Notify `gopls` of an unsaved text buffer update.
    pub async fn did_change(&self, file_uri: &str, text: &str, version: i32) -> Result<()> {
        self.send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {
                    "uri": file_uri,
                    "version": version
                },
                "contentChanges": [
                    { "text": text }
                ]
            }),
        )
        .await
    }

    /// Notify `gopls` that a document was closed.
    pub async fn did_close(&self, file_uri: &str) -> Result<()> {
        self.send_notification(
            "textDocument/didClose",
            serde_json::json!({
                "textDocument": {
                    "uri": file_uri
                }
            }),
        )
        .await
    }

    /// Query symbol definition.
    pub async fn definition(
        &self,
        file_uri: &str,
        line: u32,
        col: u32,
    ) -> Result<Option<serde_json::Value>> {
        let resp = self
            .send_request(
                "textDocument/definition",
                serde_json::json!({
                    "textDocument": { "uri": file_uri },
                    "position": { "line": line, "character": col }
                }),
            )
            .await?;

        Ok(resp.get("result").cloned())
    }

    /// Query hover documentation and type signatures.
    pub async fn hover(
        &self,
        file_uri: &str,
        line: u32,
        col: u32,
    ) -> Result<Option<serde_json::Value>> {
        let resp = self
            .send_request(
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": file_uri },
                    "position": { "line": line, "character": col }
                }),
            )
            .await?;

        Ok(resp.get("result").cloned())
    }

    /// Query all references to a symbol across the Go workspace.
    pub async fn references(
        &self,
        file_uri: &str,
        line: u32,
        col: u32,
    ) -> Result<serde_json::Value> {
        let resp = self
            .send_request(
                "textDocument/references",
                serde_json::json!({
                    "textDocument": { "uri": file_uri },
                    "position": { "line": line, "character": col },
                    "context": { "includeDeclaration": true }
                }),
            )
            .await?;

        Ok(resp.get("result").cloned().unwrap_or(serde_json::json!([])))
    }

    /// Query document symbols outline.
    pub async fn document_symbols(&self, file_uri: &str) -> Result<serde_json::Value> {
        let resp = self
            .send_request(
                "textDocument/documentSymbol",
                serde_json::json!({
                    "textDocument": { "uri": file_uri }
                }),
            )
            .await?;

        if let Some(err) = resp.get("error") {
            anyhow::bail!("gopls documentSymbol failed: {err}");
        }
        Ok(resp.get("result").cloned().unwrap_or(serde_json::json!([])))
    }

    /// Subscribe to background server notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.broadcast_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_gopls_binary_discovery() {
        let found = find_gopls_binary(None);
        // On systems with go/bin/gopls installed, verify it resolves
        if let Some(path) = &found {
            assert!(path.exists());
        }
    }

    #[tokio::test]
    async fn test_go_engine_lifecycle_if_available() {
        let gopls = find_gopls_binary(None);
        if gopls.is_none() {
            eprintln!("Skipping GoEngine live test: gopls not found in PATH or standard dirs");
            return;
        }

        let dir = tempdir().unwrap();
        let go_mod = "module example.com/demo\n\ngo 1.22\n";
        let main_go = r#"package main

import "fmt"

func Greet(name string) string {
	return fmt.Sprintf("Hello, %s!", name)
}

func main() {
	msg := Greet("World")
	fmt.Println(msg)
}
"#;
        std::fs::write(dir.path().join("go.mod"), go_mod).unwrap();
        std::fs::write(dir.path().join("main.go"), main_go).unwrap();

        let config = GoConfig::default();
        let engine = GoEngine::load(dir.path(), config).await.unwrap();

        let file_path = dir.path().join("main.go");
        let file_uri = format!("file://{}", file_path.to_string_lossy());

        // 1. didOpen
        engine.did_open(&file_uri, main_go).await.unwrap();

        // 2. Document symbols
        let symbols = engine.document_symbols(&file_uri).await.unwrap();
        assert!(symbols.is_array());
        let sym_arr = symbols.as_array().unwrap();
        assert!(!sym_arr.is_empty(), "Expected symbols in main.go");

        // 3. Hover on 'Greet'
        let hover = engine.hover(&file_uri, 4, 6).await.unwrap();
        assert!(hover.is_some(), "Expected hover documentation for Greet");

        // 4. Definition of 'Greet'
        let def = engine.definition(&file_uri, 9, 8).await.unwrap();
        assert!(def.is_some(), "Expected definition jump for Greet");

        // 5. References of 'Greet'
        let refs = engine.references(&file_uri, 4, 6).await.unwrap();
        assert!(refs.is_array());
        assert!(refs.as_array().unwrap().len() >= 2);
    }
}
