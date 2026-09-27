//! Managed Go language intelligence engine powered by supervised `gopls`.
//!
//! Provides multi-worktree Go code intelligence with shared GOCACHE and GOMODCACHE
//! for high-performance symbol resolution and compilation reuse.

use anyhow::{Context, Result};
use prod_code_protocol::readiness::{
    BUSY_MEMBER, Busy, INDEX_WAIT, Readiness, ReadySignal, needs_index,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard, Weak};
use std::time::Duration;
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
/// gopls v0.23.0 sends a complete diagnostic report with an empty discriminator (#475).
/// Canonicalize only that shape. Missing/malformed items and unchanged reports still reach the
/// client unchanged so its strict evidence checks can reject them; errors are never hidden.
fn normalize_diagnostic_response(method: &str, reply: &mut serde_json::Value) {
    if method == "textDocument/diagnostic"
        && reply.get("error").is_none()
        && reply["result"]["kind"].as_str() == Some("")
        && reply["result"]["items"].is_array()
    {
        reply["result"]["kind"] = serde_json::json!("full");
    }
}

pub struct GoEngine {
    workspace_root: PathBuf,
    stdin: Arc<Mutex<ChildStdin>>,
    next_req_id: AtomicU64,
    pending_requests: Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    pub capabilities: Arc<RwLock<Option<serde_json::Value>>>,
    broadcast_tx: broadcast::Sender<String>,
    /// False once gopls's output has ended: it exited or crashed (#355).
    is_alive: Arc<AtomicBool>,
    /// What gopls has said about loading its packages: "Setting up workspace" is begun and
    /// ended as progress (#391).
    readiness: Arc<Readiness>,
    request_timeout: Duration,
    _child: Arc<StdMutex<Child>>,
}

struct PendingRequest {
    id: u64,
    pending: Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    child: Arc<StdMutex<Child>>,
    is_alive: Arc<AtomicBool>,
    frame_written: bool,
}

struct FrameWrite {
    child: Weak<StdMutex<Child>>,
    pending: Weak<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    is_alive: Weak<AtomicBool>,
    started: bool,
    complete: bool,
}

impl FrameWrite {
    fn retire(&self) {
        if let Some(is_alive) = self.is_alive.upgrade() {
            is_alive.store(false, Ordering::Release);
        }
        if let Some(child) = self.child.upgrade() {
            let _ = lock_unpoisoned(&child).start_kill();
        }
        if let Some(pending) = self.pending.upgrade() {
            lock_unpoisoned(&pending).clear();
        }
    }
}

impl Drop for FrameWrite {
    fn drop(&mut self) {
        if self.started && !self.complete {
            self.retire();
        }
    }
}

impl PendingRequest {
    fn retire_if_partial(&self) {
        if self.frame_written {
            return;
        }
        self.is_alive.store(false, Ordering::Release);
        let _ = lock_unpoisoned(&self.child).start_kill();
        lock_unpoisoned(&self.pending).clear();
    }
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        lock_unpoisoned(&self.pending).remove(&self.id);
        self.retire_if_partial();
    }
}

fn lock_unpoisoned<T>(mutex: &StdMutex<T>) -> StdMutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl GoEngine {
    /// Launch a new managed Go engine for the given workspace root.
    pub async fn load(workspace_root: &Path, config: GoConfig) -> Result<Self> {
        Self::load_with_request_timeout(workspace_root, config, Duration::from_secs(30)).await
    }

    async fn load_with_request_timeout(
        workspace_root: &Path,
        config: GoConfig,
        request_timeout: Duration,
    ) -> Result<Self> {
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

        let pending_requests: Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let pending_clone = pending_requests.clone();

        let child = Arc::new(StdMutex::new(child));
        let child_writer = Arc::downgrade(&child);
        let pending_writer = Arc::downgrade(&pending_requests);
        let stdin_arc = Arc::new(Mutex::new(stdin));
        let stdin_writer = stdin_arc.clone();
        let is_alive = Arc::new(AtomicBool::new(true));
        let is_alive_reader = Arc::clone(&is_alive);
        let readiness = Arc::new(Readiness::new(ReadySignal::Progress));
        let readiness_reader = Arc::clone(&readiness);
        let auto_reply_timeout = request_timeout;

        // Background reader loop: decodes LSP frames and routes responses to oneshot channels
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut header_line = String::new();

            'reader: loop {
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
                                    readiness_reader.on_message(&val);
                                    // 1. Check if this is a response to our pending request
                                    if let Some(id_val) = val.get("id") {
                                        // Only an answer is ours: gopls numbers its own requests
                                        // (`window/workDoneProgress/create`) from 1 too (#391).
                                        if val.get("method").is_none()
                                            && let Some(id) = id_val.as_u64()
                                        {
                                            let mut pending = lock_unpoisoned(&pending_clone);
                                            if let Some(tx) = pending.remove(&id) {
                                                let _ = tx.send(val.clone());
                                                continue;
                                            }
                                        }

                                        // Server-initiated request requiring auto-reply
                                        let method = val.get("method").and_then(|m| m.as_str());
                                        if let Some(m) = method {
                                            let auto_resp = match m {
                                                "window/workDoneProgress/create"
                                                | "client/registerCapability" => {
                                                    serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": id_val,
                                                        "result": null
                                                    })
                                                }
                                                "workspace/configuration" => {
                                                    serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": id_val,
                                                        "result": [{}]
                                                    })
                                                }
                                                _ => serde_json::json!({
                                                    "jsonrpc": "2.0",
                                                    "id": id_val,
                                                    "error": { "code": -32601, "message": format!("{m} is not supported by prod-code") }
                                                }),
                                            };
                                            if let Err(error) = Self::write_frame_until(
                                                &stdin_writer,
                                                &auto_resp,
                                                tokio::time::Instant::now() + auto_reply_timeout,
                                                m,
                                                &child_writer,
                                                &pending_writer,
                                                &Arc::downgrade(&is_alive_reader),
                                            )
                                            .await
                                            {
                                                tracing::warn!(
                                                    method = m,
                                                    error = %error,
                                                    "failed to answer gopls request; retiring process"
                                                );
                                                let mut retirement = FrameWrite {
                                                    child: child_writer.clone(),
                                                    pending: pending_writer.clone(),
                                                    is_alive: Arc::downgrade(&is_alive_reader),
                                                    started: true,
                                                    complete: false,
                                                };
                                                retirement.retire();
                                                retirement.complete = true;
                                                break 'reader;
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
            is_alive_reader.store(false, Ordering::Relaxed);
            // No answer is coming for a request still waiting: dropping its sender ends the
            // wait now, not at the timeout (#355).
            lock_unpoisoned(&pending_clone).clear();
            if let Some(child) = child_writer.upgrade() {
                let _ = lock_unpoisoned(&child).start_kill();
                for _ in 0..200 {
                    let status = {
                        let mut child = lock_unpoisoned(&child);
                        child.try_wait()
                    };
                    match status {
                        Ok(Some(_)) | Err(_) => break,
                        Ok(None) => tokio::time::sleep(Duration::from_millis(5)).await,
                    }
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
            is_alive,
            readiness,
            request_timeout,
            _child: child,
        };

        // Initialize gopls with workspace root
        engine.initialize().await?;

        Ok(engine)
    }

    /// Helper to write an LSP Content-Length frame directly to child stdin.
    async fn write_frame_until(
        writer: &Arc<Mutex<ChildStdin>>,
        val: &serde_json::Value,
        deadline: tokio::time::Instant,
        method: &str,
        child: &Weak<StdMutex<Child>>,
        pending: &Weak<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
        is_alive: &Weak<AtomicBool>,
    ) -> Result<()> {
        let body = val.to_string();
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut sin = tokio::time::timeout_at(deadline, writer.lock())
            .await
            .with_context(|| format!("Timeout waiting to send gopls message '{method}'"))?;
        if !is_alive
            .upgrade()
            .is_some_and(|alive| alive.load(Ordering::Acquire))
        {
            anyhow::bail!("gopls process has exited before message '{method}'");
        }
        // Declared after `sin`, so cancellation retires the process before unlocking stdin.
        let mut frame_write = FrameWrite {
            child: child.clone(),
            pending: pending.clone(),
            is_alive: is_alive.clone(),
            started: true,
            complete: false,
        };
        tokio::time::timeout_at(deadline, sin.write_all(frame.as_bytes()))
            .await
            .with_context(|| format!("Timeout writing gopls message '{method}'"))?
            .with_context(|| format!("Failed to write gopls message '{method}'"))?;
        tokio::time::timeout_at(deadline, sin.flush())
            .await
            .with_context(|| format!("Timeout flushing gopls message '{method}'"))?
            .with_context(|| format!("Failed to flush gopls message '{method}'"))?;
        frame_write.complete = true;
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
                // gopls reports loading its packages as progress to a client that takes it.
                "window": {
                    "workDoneProgress": true
                },
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
        self.readiness.started();

        Ok(resp)
    }

    /// The package loading gopls is still doing, if any (#391).
    pub fn busy(&self) -> Option<Busy> {
        self.readiness.busy()
    }

    /// Send a typed JSON-RPC request and wait for the response.
    pub async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        if !self.is_alive.load(Ordering::Acquire) {
            anyhow::bail!("gopls process has exited before request '{method}'");
        }
        // A question answered from the index waits until gopls has loaded its packages (#391).
        let busy = if needs_index(method) {
            self.readiness.wait(INDEX_WAIT).await
        } else {
            None
        };
        let deadline = tokio::time::Instant::now() + self.request_timeout;
        let req_id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params
        });

        let body = payload.to_string();
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut writer = tokio::time::timeout_at(deadline, self.stdin.lock())
            .await
            .with_context(|| format!("Timeout waiting to send gopls request '{method}'"))?;
        if !self.is_alive.load(Ordering::Acquire) {
            anyhow::bail!("gopls process has exited before request '{method}'");
        }
        let (tx, rx) = oneshot::channel();
        lock_unpoisoned(&self.pending_requests).insert(req_id, tx);
        // Declared after `writer`: cancellation or a write error retires the partial stream
        // while this request still owns the writer lock, before a queued writer can wake.
        let mut pending = PendingRequest {
            id: req_id,
            pending: Arc::clone(&self.pending_requests),
            child: Arc::clone(&self._child),
            is_alive: Arc::clone(&self.is_alive),
            frame_written: false,
        };
        tokio::time::timeout_at(deadline, writer.write_all(frame.as_bytes()))
            .await
            .with_context(|| format!("Timeout writing gopls request '{method}'"))?
            .with_context(|| format!("Failed to write gopls request '{method}'"))?;
        tokio::time::timeout_at(deadline, writer.flush())
            .await
            .with_context(|| format!("Timeout flushing gopls request '{method}'"))?
            .with_context(|| format!("Failed to flush gopls request '{method}'"))?;
        pending.frame_written = true;
        drop(writer);

        match tokio::time::timeout_at(deadline, rx).await {
            Ok(Ok(mut val)) => {
                if !self.is_alive.load(Ordering::Acquire) {
                    anyhow::bail!("gopls has exited while answering '{method}'");
                }
                normalize_diagnostic_response(method, &mut val);
                if let Some(busy) = busy {
                    val[BUSY_MEMBER] = serde_json::to_value(busy)?;
                }
                Ok(val)
            }
            Ok(Err(_)) => anyhow::bail!("gopls has exited while answering '{method}'"),
            Err(_) => {
                anyhow::bail!("Timeout waiting for gopls response to method '{method}'");
            }
        }
    }

    /// Whether gopls is still running: false once its output has ended (#355).
    pub fn is_alive(&self) -> bool {
        self.is_alive.load(Ordering::Relaxed)
    }

    /// Send an asynchronous JSON-RPC notification to `gopls`.
    pub async fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        if !self.is_alive.load(Ordering::Acquire) {
            anyhow::bail!("gopls process has exited before notification '{method}'");
        }
        let deadline = tokio::time::Instant::now() + self.request_timeout;
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        Self::write_frame_until(
            &self.stdin,
            &payload,
            deadline,
            method,
            &Arc::downgrade(&self._child),
            &Arc::downgrade(&self.pending_requests),
            &Arc::downgrade(&self.is_alive),
        )
        .await
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

    #[cfg(unix)]
    const FAKE_GOPLS: &str = r#"#!/usr/bin/env python3
import json, os, sys, threading, time

LOCK = threading.Lock()
SEEN = []
SEEN_FILE = os.environ.get("FAKE_SEEN_FILE")
if os.environ.get("FAKE_PID_FILE"):
    with open(os.environ["FAKE_PID_FILE"], "w") as pid_file:
        pid_file.write(str(os.getpid()))

def send(message):
    body = json.dumps(message).encode()
    with LOCK:
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
        sys.stdout.buffer.write(body)
        sys.stdout.buffer.flush()

def read():
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    body = b""
    while len(body) < length:
        chunk = sys.stdin.buffer.read(min(length - len(body), 4096))
        if not chunk:
            return None
        body += chunk
        if os.environ.get("FAKE_SLOW_READ"):
            time.sleep(float(os.environ["FAKE_SLOW_READ"]))
    return json.loads(body)

while True:
    message = read()
    if message is None:
        break
    method = message.get("method", "")
    SEEN.append(method)
    if SEEN_FILE:
        with open(SEEN_FILE, "a") as seen:
            seen.write(method + "\n")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {"hoverProvider": True}}})
    elif method == "initialized" and os.environ.get("FAKE_CLOSE_STDIN"):
        os.close(0)
        threading.Event().wait()
    elif method == "initialized" and os.environ.get("FAKE_HUGE_AUTO_REQUEST"):
        send({"jsonrpc": "2.0", "id": 7001, "method": "x" * (8 * 1024 * 1024), "params": {}})
        threading.Event().wait()
    elif method == "initialized" and os.environ.get("FAKE_STOP_READING"):
        threading.Event().wait()
    elif method == "textDocument/hover":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"contents": "healthy"}})
    elif method == "prodCode/delay":
        threading.Timer(0.4, send, ({"jsonrpc": "2.0", "id": message["id"], "result": "late"},)).start()
    elif method == "prodCode/seen":
        send({"jsonrpc": "2.0", "id": message["id"], "result": SEEN})
    elif method == "prodCode/queuedAfterRetirement":
        send({"jsonrpc": "2.0", "id": message["id"], "result": "unexpected success"})
"#;

    #[cfg(unix)]
    async fn fake_engine(
        mode: Option<(&str, &str)>,
        timeout: Duration,
    ) -> (tempfile::TempDir, Arc<GoEngine>) {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().expect("tempdir");
        let script = dir.path().join("fake-gopls");
        std::fs::write(&script, FAKE_GOPLS).expect("write fake gopls");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make fake gopls executable");
        let mut config = GoConfig {
            gopls_path: Some(script),
            shared_cache_dir: Some(dir.path().join("cache")),
            ..Default::default()
        };
        config.extra_env.insert(
            "FAKE_SEEN_FILE".to_string(),
            dir.path().join("seen").to_string_lossy().into_owned(),
        );
        config.extra_env.insert(
            "FAKE_PID_FILE".to_string(),
            dir.path().join("pid").to_string_lossy().into_owned(),
        );
        if let Some((name, value)) = mode {
            config.extra_env.insert(name.to_string(), value.to_string());
        }
        let engine = GoEngine::load_with_request_timeout(dir.path(), config, timeout)
            .await
            .expect("the fake gopls starts");
        (dir, Arc::new(engine))
    }

    #[cfg(unix)]
    async fn assert_process_exits(pid_file: &Path) {
        let pid = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(pid) = std::fs::read_to_string(pid_file) {
                    break pid;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the fake server records its pid");
        tokio::time::timeout(Duration::from_secs(2), async {
            while std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .status()
                .is_ok_and(|status| status.success())
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the owned fake server exits");
    }

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

    /// The first `workspace/symbol` of a fresh gopls waits for its package loading and finds
    /// the function, however soon it is asked: taken as it came, it was empty (#391).
    #[tokio::test]
    async fn the_first_symbol_search_of_a_fresh_gopls_finds_the_function() {
        if find_gopls_binary(None).is_none() {
            eprintln!("Skipping: gopls not found");
            return;
        }
        // Not `tempdir()`: its `.tmp` name is a directory Go tools skip, and gopls then finds the
        // module's symbols in no package, only in the opened file once it has loaded it.
        let dir = tempfile::Builder::new()
            .prefix("prod-code-go-")
            .tempdir()
            .unwrap();
        std::fs::write(
            dir.path().join("go.mod"),
            "module example.com/subject\n\ngo 1.22\n",
        )
        .unwrap();
        let store = "package subject\n\n// Total sums the quantities.\nfunc Total(all []int) int {\n\tsum := 0\n\tfor _, q := range all {\n\t\tsum += q\n\t}\n\treturn sum\n}\n";
        std::fs::write(dir.path().join("store.go"), store).unwrap();
        let engine = GoEngine::load(dir.path(), GoConfig::default())
            .await
            .unwrap();
        let uri = format!("file://{}", dir.path().join("store.go").to_string_lossy());
        engine.did_open(&uri, store).await.unwrap();
        let answer = engine
            .send_request("workspace/symbol", serde_json::json!({ "query": "Total" }))
            .await
            .unwrap();
        let hits = answer["result"].as_array().map(Vec::len).unwrap_or(0);
        assert!(hits >= 1, "{answer}");
        assert_eq!(engine.busy(), None);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_deadline_covers_a_blocked_full_frame_write() {
        let (_dir, engine) =
            fake_engine(Some(("FAKE_STOP_READING", "1")), Duration::from_millis(200)).await;
        let observed = tokio::time::timeout(
            Duration::from_secs(2),
            engine.send_request(
                "prodCode/large",
                serde_json::json!({ "payload": "x".repeat(8 * 1024 * 1024) }),
            ),
        )
        .await;
        let error = observed
            .expect("the internal deadline includes a blocked write")
            .expect_err("the full frame cannot be written");
        let text = format!("{error:#}");
        assert!(text.contains("prodCode/large"), "{text}");
        assert!(text.to_lowercase().contains("timeout"), "{text}");
        assert!(!engine.is_alive(), "the partial stream is retired");
        assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn writer_lock_timeout_never_sends_the_expired_request() {
        let (dir, engine) = fake_engine(None, Duration::from_millis(100)).await;
        let writer = engine.stdin.lock().await;
        let waiting = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .send_request("prodCode/queued", serde_json::json!({}))
                    .await
            })
        };
        let error = waiting
            .await
            .expect("request task")
            .expect_err("the request expires behind the writer");
        let text = format!("{error:#}");
        assert!(text.contains("prodCode/queued"), "{text}");
        assert!(text.to_lowercase().contains("timeout"), "{text}");
        drop(writer);

        let seen = engine
            .send_request("prodCode/seen", serde_json::json!({}))
            .await
            .expect("healthy response after contention");
        assert!(
            !seen["result"]
                .as_array()
                .expect("methods")
                .iter()
                .any(|method| method == "prodCode/queued"),
            "{seen}"
        );
        assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
        let methods = std::fs::read_to_string(dir.path().join("seen")).expect("request log");
        assert!(!methods.lines().any(|method| method == "prodCode/queued"));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_request_rechecks_retirement_before_writing() {
        let (dir, engine) = fake_engine(None, Duration::from_secs(2)).await;
        let writer = engine.stdin.lock().await;
        let before = engine.next_req_id.load(Ordering::Relaxed);
        let waiting = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .send_request("prodCode/queuedAfterRetirement", serde_json::json!({}))
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            while engine.next_req_id.load(Ordering::Relaxed) == before {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the request reached the occupied writer");

        engine.is_alive.store(false, Ordering::Release);
        drop(writer);
        let error = waiting
            .await
            .expect("request task")
            .expect_err("a retired engine cannot answer successfully");
        assert!(
            format!("{error:#}").contains("exited before request"),
            "{error:#}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        let seen = std::fs::read_to_string(dir.path().join("seen")).expect("request log");
        assert!(
            !seen
                .lines()
                .any(|method| method == "prodCode/queuedAfterRetirement"),
            "{seen}"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn write_error_keeps_context_and_retires_the_child() {
        let (_dir, engine) =
            fake_engine(Some(("FAKE_CLOSE_STDIN", "1")), Duration::from_secs(2)).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let error = engine
            .send_request(
                "prodCode/brokenWrite",
                serde_json::json!({ "payload": "x".repeat(1024 * 1024) }),
            )
            .await
            .expect_err("stdin was closed");
        let text = format!("{error:#}");
        assert!(text.contains("prodCode/brokenWrite"), "{text}");
        assert!(text.to_lowercase().contains("write"), "{text}");
        assert!(!engine.is_alive());
        assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_a_partial_frame_retires_the_child_and_wakes_waiters() {
        let (_dir, engine) =
            fake_engine(Some(("FAKE_STOP_READING", "1")), Duration::from_secs(10)).await;
        let writing = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .send_request(
                        "prodCode/cancelled",
                        serde_json::json!({ "payload": "x".repeat(8 * 1024 * 1024) }),
                    )
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        let waiting = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .send_request("prodCode/waiting", serde_json::json!({}))
                    .await
            })
        };
        writing.abort();
        assert!(writing.await.expect_err("cancelled").is_cancelled());
        let error = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("the waiter is woken")
            .expect("waiter task")
            .expect_err("the child was retired");
        assert!(
            format!("{error:#}").contains("prodCode/waiting"),
            "{error:#}"
        );
        assert!(!engine.is_alive());
        assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn complete_frame_cancellation_cleans_pending_and_keeps_concurrency_healthy() {
        let (_dir, engine) = fake_engine(None, Duration::from_secs(2)).await;
        let cancelled = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .send_request("prodCode/delay", serde_json::json!({}))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancelled.abort();
        assert!(cancelled.await.expect_err("cancelled").is_cancelled());
        assert!(lock_unpoisoned(&engine.pending_requests).is_empty());

        let first = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .send_request("textDocument/hover", serde_json::json!({}))
                    .await
            })
        };
        let second = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .send_request("textDocument/hover", serde_json::json!({}))
                    .await
            })
        };
        for response in [first, second] {
            assert_eq!(
                response.await.expect("task").expect("response")["result"]["contents"],
                "healthy"
            );
        }
        assert!(engine.is_alive());
        assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notification_deadline_covers_a_blocked_frame() {
        let (dir, engine) =
            fake_engine(Some(("FAKE_STOP_READING", "1")), Duration::from_millis(200)).await;
        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            engine.send_notification("textDocument/didOpen", serde_json::json!({
                "textDocument": {"uri": "file:///notification.go", "languageId":"go", "version":1, "text": "x".repeat(8 * 1024 * 1024)}
            })),
        ).await.expect("notification must honor its internal write budget");
        let error = outcome.expect_err("the server never reads the notification");
        let text = format!("{error:#}");
        assert!(text.contains("textDocument/didOpen"), "{text}");
        assert!(text.to_lowercase().contains("timeout"), "{text}");
        assert!(
            !engine.is_alive(),
            "a partial document frame cannot be reused"
        );
        assert_process_exits(&dir.path().join("pid")).await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_a_notification_retires_its_owned_process() {
        let (dir, engine) =
            fake_engine(Some(("FAKE_STOP_READING", "1")), Duration::from_secs(10)).await;
        let writing = {
            let engine = Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .send_notification(
                        "prodCode/cancelledNotification",
                        serde_json::json!({"payload": "x".repeat(8 * 1024 * 1024)}),
                    )
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        writing.abort();
        assert!(writing.await.expect_err("cancelled").is_cancelled());
        assert!(!engine.is_alive());
        assert_process_exits(&dir.path().join("pid")).await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_automatic_response_retires_gopls() {
        let (dir, engine) = fake_engine(
            Some(("FAKE_HUGE_AUTO_REQUEST", "1")),
            Duration::from_millis(200),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while engine.is_alive() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the automatic response has a bounded write");
        assert_process_exits(&dir.path().join("pid")).await;
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn complete_notification_keeps_gopls_healthy() {
        let (_dir, engine) = fake_engine(None, Duration::from_secs(2)).await;
        engine
            .send_notification("workspace/didChangeConfiguration", serde_json::json!({}))
            .await
            .unwrap();
        let response = engine
            .send_request("textDocument/hover", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(response["result"]["contents"], "healthy");
        assert!(engine.is_alive());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_engine_kills_a_reader_owned_server() {
        let (dir, engine) = fake_engine(None, Duration::from_secs(2)).await;
        drop(engine);
        assert_process_exits(&dir.path().join("pid")).await;
    }
}

#[cfg(test)]
mod diagnostic_report_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_complete_empty_kind_diagnostic_reports_are_normalized() {
        for items in [json!([]), json!([{"message":"an error","severity":1}])] {
            let mut reply = json!({"jsonrpc":"2.0","id":4,"result":{"kind":"","items":items,"resultId":"keep"}});
            normalize_diagnostic_response("textDocument/diagnostic", &mut reply);
            assert_eq!(reply["result"]["kind"], "full");
            assert_eq!(reply["result"]["items"], items);
            assert_eq!(reply["result"]["resultId"], "keep");
            assert_eq!(reply["id"], 4);
        }
        for original in [
            json!(null),
            json!({"result":null}),
            json!({"result":{"kind":""}}),
            json!({"result":{"kind":"","items":null}}),
            json!({"result":{"kind":"","items":{}}}),
            json!({"result":{"kind":"unchanged","resultId":"old","items":[]}}),
            json!({"result":{"kind":"mystery","items":[]}}),
            json!({"error":{"code":-32603},"result":{"kind":"","items":[]}}),
        ] {
            let mut reply = original.clone();
            normalize_diagnostic_response("textDocument/diagnostic", &mut reply);
            assert_eq!(reply, original);
        }
        let original = json!({"result":{"kind":"","items":[]}});
        let mut reply = original.clone();
        normalize_diagnostic_response("textDocument/hover", &mut reply);
        assert_eq!(reply, original);
    }

    #[tokio::test]
    async fn real_gopls_diagnostics_are_full_and_preserve_actual_errors() {
        let gopls = find_gopls_binary(None).expect("real gopls is required for this regression");
        let dir = tempfile::Builder::new()
            .prefix("go-diagnostic-report-")
            .tempdir()
            .unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let source = "package subject\n\nfunc Value() int { return 1 }\n";
        std::fs::write(
            root.join("go.mod"),
            "module example.com/diagnostics\n\ngo 1.22\n",
        )
        .unwrap();
        std::fs::write(root.join("value.go"), source).unwrap();
        let engine = GoEngine::load(
            &root,
            GoConfig {
                gopls_path: Some(gopls),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let uri = url::Url::from_file_path(root.join("value.go"))
            .unwrap()
            .to_string();
        engine.did_open(&uri, source).await.unwrap();
        let ready = engine
            .send_request("workspace/symbol", json!({"query":"Value"}))
            .await
            .unwrap();
        assert!(!ready["result"].as_array().unwrap().is_empty(), "{ready}");
        for (version, text, error) in [
            (
                2,
                "package subject\n\nfunc Value() int { return \"bad\" }\n",
                true,
            ),
            (3, source, false),
        ] {
            engine.did_change(&uri, text, version).await.unwrap();
            let answer = engine
                .send_request(
                    "textDocument/diagnostic",
                    json!({"textDocument":{"uri":uri}}),
                )
                .await
                .unwrap();
            eprintln!("version {version}: {answer}");
            assert!(answer.get("error").is_none(), "{answer}");
            assert_eq!(answer["result"]["kind"], "full", "{answer}");
            let items = answer["result"]["items"].as_array().unwrap();
            assert_eq!(items.iter().any(|d| d["severity"] == 1), error, "{answer}");
            if error {
                assert!(
                    items.iter().any(|d| d["message"]
                        .as_str()
                        .is_some_and(|m| m.contains("string") && m.contains("int"))),
                    "{answer}"
                );
            }
        }
    }
}
