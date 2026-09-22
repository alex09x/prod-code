//! Pluggable Generic LSP engine for external language servers (Pyright, Ruff, TypeScript, etc.).
//!
//! Provides supervised process lifecycle, automatic framing, request/response routing,
//! health monitoring, and idle shutdown management.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, RwLock, broadcast, oneshot};

/// Configuration for a generic LSP server adapter.
#[derive(Debug, Clone)]
pub struct GenericLspConfig {
    /// Command / binary name or path to execute (e.g. `pyright-langserver`, `ruff`, `vtsls`).
    pub command: String,
    /// Arguments to pass to the binary (e.g. `["--stdio"]`).
    pub args: Vec<String>,
    /// Environment variables to pass to the child process.
    pub env: HashMap<String, String>,
    /// Working directory for the server.
    pub working_dir: Option<PathBuf>,
    /// `initializationOptions` sent with the LSP `initialize` request.
    pub initialization_options: Option<serde_json::Value>,
    /// How long to wait for an answer before giving up on a request. Servers differ by more
    /// than an order of magnitude — a formatter answers instantly, a type checker on a cold
    /// project does not — so this is per server rather than one number for all of them.
    pub request_timeout: Duration,
}

impl Default for GenericLspConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            working_dir: None,
            initialization_options: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

/// What a request waits when the configuration says nothing.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The native TypeScript 7 compiler binary, which doubles as the language server
/// (`tsc --lsp --stdio`): `tsgo` on PATH, or the platform package under the global
/// `typescript` install (`@typescript/typescript-<os>-<arch>/lib/tsc`).
fn native_typescript_lsp() -> Option<PathBuf> {
    if let Ok(tsgo) = which_bin("tsgo") {
        return Some(tsgo);
    }
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let candidate = npm_global_root()?
        .join("typescript")
        .join("node_modules")
        .join("@typescript")
        .join(format!("typescript-{os}-{arch}"))
        .join("lib")
        .join("tsc");
    candidate.is_file().then_some(candidate)
}

/// Global npm module root (`npm root -g`), where `npm install -g` puts packages.
fn npm_global_root() -> Option<PathBuf> {
    let out = std::process::Command::new("npm")
        .args(["root", "-g"])
        .output()
        .ok()?;
    let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!root.is_empty()).then(|| PathBuf::from(root))
}

impl GenericLspConfig {
    /// The language server this host would run for `engine` (`cpp`, `swift`, `python`,
    /// `typescript`), as a short label for the gateway status, or `None` when none of the
    /// candidates is installed.
    pub fn installed_server(engine: &str) -> Option<String> {
        let config = match engine {
            "cpp" => Self::for_cpp(),
            "swift" => Self::for_swift(),
            "python" => Self::for_python(),
            "typescript" => Self::for_typescript(),
            _ => return None,
        };
        let command = Path::new(&config.command);
        let installed = if command.is_absolute() {
            command.is_file()
        } else if config.command == "xcrun" {
            std::process::Command::new("xcrun")
                .args(["--find", "sourcekit-lsp"])
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
        } else {
            which_bin(&config.command).is_ok()
        };
        if !installed {
            return None;
        }
        let label = match command.file_name().and_then(|n| n.to_str()) {
            Some("tsc") | Some("tsgo") => "tsc --lsp".to_string(),
            Some("xcrun") => "sourcekit-lsp".to_string(),
            Some(name) => name.to_string(),
            None => config.command.clone(),
        };
        Some(label)
    }

    /// Create a standard configuration for Python language servers.
    pub fn for_python() -> Self {
        let (cmd, args) = if which_bin("basedpyright-langserver").is_ok() {
            (
                "basedpyright-langserver".to_string(),
                vec!["--stdio".to_string()],
            )
        } else if which_bin("pyright-langserver").is_ok() {
            (
                "pyright-langserver".to_string(),
                vec!["--stdio".to_string()],
            )
        } else if which_bin("ruff").is_ok() {
            ("ruff".to_string(), vec!["server".to_string()])
        } else {
            ("pylsp".to_string(), vec![])
        };

        Self {
            command: cmd,
            args,
            env: HashMap::new(),
            working_dir: None,
            initialization_options: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Create a configuration for C/C++ (clangd). A `compile_commands.json` at the workspace
    /// root or under `build/` gives clangd the real flags.
    pub fn for_cpp() -> Self {
        Self {
            command: "clangd".to_string(),
            args: vec![
                "--background-index".to_string(),
                "--header-insertion=never".to_string(),
                "--log=error".to_string(),
                "--compile-commands-dir=build".to_string(),
            ],
            env: HashMap::new(),
            working_dir: None,
            initialization_options: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Create a configuration for Swift (sourcekit-lsp). On macOS the toolchain's server is
    /// reached through `xcrun` when it is not on PATH.
    pub fn for_swift() -> Self {
        let (cmd, args) = if which_bin("sourcekit-lsp").is_ok() {
            ("sourcekit-lsp".to_string(), vec![])
        } else if cfg!(target_os = "macos") {
            ("xcrun".to_string(), vec!["sourcekit-lsp".to_string()])
        } else {
            ("sourcekit-lsp".to_string(), vec![])
        };
        Self {
            command: cmd,
            args,
            env: HashMap::new(),
            working_dir: None,
            initialization_options: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Create a standard configuration for TypeScript / JavaScript language servers.
    pub fn for_typescript() -> Self {
        // TypeScript 7 (native) ships its own LSP: `tsc --lsp --stdio` from the platform
        // package. It needs no tsserver and no Node at all, so it wins when present.
        let (cmd, args) = if let Some(native) = native_typescript_lsp() {
            (
                native.to_string_lossy().into_owned(),
                vec!["--lsp".to_string(), "--stdio".to_string()],
            )
        } else if which_bin("typescript-language-server").is_ok() {
            (
                "typescript-language-server".to_string(),
                vec!["--stdio".to_string()],
            )
        } else if which_bin("vtsls").is_ok() {
            ("vtsls".to_string(), vec!["--stdio".to_string()])
        } else {
            (
                "typescript-language-server".to_string(),
                vec!["--stdio".to_string()],
            )
        };
        // typescript-language-server does not bundle TypeScript: a workspace without
        // node_modules/typescript needs the global install pointed at explicitly.
        let initialization_options = npm_global_root()
            .map(|root| root.join("typescript").join("lib"))
            .filter(|lib| lib.join("tsserver.js").exists())
            .map(|lib| {
                serde_json::json!({
                    "tsserver": { "path": lib.to_string_lossy() },
                    "preferences": { "includeInlayParameterNameHints": "none" }
                })
            });

        Self {
            command: cmd,
            args,
            env: HashMap::new(),
            working_dir: None,
            initialization_options,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

/// The settings a language server gets for a `workspace/configuration` section. Pyright
/// analyses the whole workspace (so rename and references cover files nobody opened) and
/// uses the checkout's virtual environment when there is one; everything else gets `{}`.
pub fn settings_for_section(root: &Path, section: &str) -> serde_json::Value {
    let analysis = serde_json::json!({
        "diagnosticMode": "workspace",
        "autoSearchPaths": true,
        "useLibraryCodeForTypes": true,
    });
    match section {
        "python.analysis" | "basedpyright.analysis" => analysis,
        s if s.starts_with("python") || s.starts_with("basedpyright") => {
            let mut settings = serde_json::json!({ "analysis": analysis });
            if let Some(python) = venv_python(root) {
                settings["pythonPath"] = serde_json::Value::String(python);
                settings["venvPath"] =
                    serde_json::Value::String(root.to_string_lossy().into_owned());
                settings["venv"] = serde_json::Value::String(".venv".to_string());
            }
            settings
        }
        _ => serde_json::json!({}),
    }
}

/// `<root>/.venv/bin/python` (or `venv/`) when the checkout carries a virtual environment.
pub fn venv_python(root: &Path) -> Option<String> {
    ["\x2evenv", "venv"]
        .iter()
        .map(|dir| {
            root.join(dir.replace("\\x2e", "."))
                .join("bin")
                .join("python")
        })
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Helper to check if an executable binary is present in PATH.
pub fn which_bin(name: &str) -> Result<PathBuf> {
    let output = std::process::Command::new("which")
        .arg(name)
        .output()
        .context("which command failed")?;
    if output.status.success() {
        let path_str = String::from_utf8(output.stdout)?.trim().to_string();
        if !path_str.is_empty() {
            return Ok(PathBuf::from(path_str));
        }
    }
    anyhow::bail!("Binary {name} not found in PATH")
}

/// Supervised generic language server process adapter.
pub struct GenericLspEngine {
    pub workspace_root: PathBuf,
    pub config: GenericLspConfig,
    stdin: Arc<Mutex<ChildStdin>>,
    next_req_id: AtomicU64,
    pending_requests: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    pub capabilities: Arc<RwLock<Option<serde_json::Value>>>,
    broadcast_tx: broadcast::Sender<String>,
    last_activity: Arc<RwLock<Instant>>,
    /// Latest `textDocument/publishDiagnostics` per document URI, the context quick fixes
    /// (`textDocument/codeAction`) are computed from.
    diagnostics: Arc<RwLock<HashMap<String, Vec<serde_json::Value>>>>,
    /// Receives the `workspace/applyEdit` a server sends while a command runs.
    apply_edit_waiter: Arc<Mutex<Option<oneshot::Sender<serde_json::Value>>>>,
    is_alive: Arc<AtomicBool>,
    _child: Arc<Mutex<Child>>,
}

impl GenericLspEngine {
    /// Spawn and initialize a generic language server for the workspace.
    pub async fn spawn(workspace_root: &Path, config: GenericLspConfig) -> Result<Self> {
        let work_dir = config
            .working_dir
            .clone()
            .unwrap_or_else(|| workspace_root.to_path_buf());

        tracing::info!(
            workspace = ?workspace_root,
            command = %config.command,
            args = ?config.args,
            "Spawning generic LSP engine"
        );

        let mut cmd = Command::new(&config.command);
        cmd.kill_on_drop(true);
        cmd.args(&config.args)
            .current_dir(&work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "Failed to execute command: {} {:?}",
                config.command, config.args
            )
        })?;

        let stdin = child
            .stdin
            .take()
            .context("Failed to open child stdin for generic LSP")?;
        let stdout = child
            .stdout
            .take()
            .context("Failed to open child stdout for generic LSP")?;

        let (bcast_tx, _) = broadcast::channel(1024);
        let bcast_tx_clone = bcast_tx.clone();
        let diagnostics: Arc<RwLock<HashMap<String, Vec<serde_json::Value>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let diagnostics_writer = diagnostics.clone();
        let apply_edit_waiter: Arc<Mutex<Option<oneshot::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(None));
        let apply_edit_slot = apply_edit_waiter.clone();

        let pending_requests: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending_requests.clone();

        let stdin_arc = Arc::new(Mutex::new(stdin));
        let stdin_writer = stdin_arc.clone();
        let config_root = workspace_root.to_path_buf();

        let is_alive = Arc::new(AtomicBool::new(true));
        let is_alive_clone = is_alive.clone();

        let last_activity = Arc::new(RwLock::new(Instant::now()));
        let activity_updater = last_activity.clone();

        // Background reader loop: decodes Content-Length frames
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut header_line = String::new();

            loop {
                header_line.clear();
                match reader.read_line(&mut header_line).await {
                    Ok(0) => break, // Process exited / EOF
                    Ok(_) => {
                        if header_line.starts_with("Content-Length:") {
                            let len_str = header_line.trim_start_matches("Content-Length:").trim();
                            if let Ok(len) = len_str.parse::<usize>() {
                                header_line.clear();
                                let _ = reader.read_line(&mut header_line).await;

                                let mut body = vec![0u8; len];
                                if reader.read_exact(&mut body).await.is_err() {
                                    continue;
                                }
                                let Ok(json_str) = String::from_utf8(body) else {
                                    continue;
                                };

                                {
                                    let mut act = activity_updater.write().await;
                                    *act = Instant::now();
                                }

                                if let Ok(val) =
                                    serde_json::from_str::<serde_json::Value>(&json_str)
                                {
                                    if let Some(id_val) = val.get("id") {
                                        if let Some(id) = id_val.as_u64() {
                                            let mut pending = pending_clone.lock().await;
                                            if let Some(tx) = pending.remove(&id) {
                                                let _ = tx.send(val.clone());
                                                continue;
                                            }
                                        }

                                        // Auto-respond to server requests
                                        let method = val.get("method").and_then(|m| m.as_str());
                                        if let Some(m) = method {
                                            match m {
                                                "window/workDoneProgress/create"
                                                | "client/registerCapability" => {
                                                    let resp = serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": id_val,
                                                        "result": null
                                                    });
                                                    let _ =
                                                        Self::write_frame_raw(&stdin_writer, &resp)
                                                            .await;
                                                }
                                                "workspace/configuration" => {
                                                    // One empty settings object per requested
                                                    // item: pyright stalls on `null` settings, and
                                                    // a short array leaves the server waiting.
                                                    let sections: Vec<String> = val
                                                        .get("params")
                                                        .and_then(|p| p.get("items"))
                                                        .and_then(|i| i.as_array())
                                                        .map(|items| {
                                                            items
                                                                .iter()
                                                                .map(|item| {
                                                                    item.get("section")
                                                                        .and_then(|s| s.as_str())
                                                                        .unwrap_or("")
                                                                        .to_string()
                                                                })
                                                                .collect()
                                                        })
                                                        .unwrap_or_else(|| vec![String::new()]);
                                                    let values: Vec<serde_json::Value> = sections
                                                        .iter()
                                                        .map(|section| {
                                                            settings_for_section(
                                                                &config_root,
                                                                section,
                                                            )
                                                        })
                                                        .collect();
                                                    let resp = serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": id_val,
                                                        "result": values
                                                    });
                                                    let _ =
                                                        Self::write_frame_raw(&stdin_writer, &resp)
                                                            .await;
                                                }
                                                "workspace/applyEdit" => {
                                                    // A command's edit: hand it to whoever is
                                                    // waiting (applyAssist) and confirm.
                                                    let edit = val
                                                        .get("params")
                                                        .and_then(|p| p.get("edit"))
                                                        .cloned()
                                                        .unwrap_or(serde_json::Value::Null);
                                                    if let Some(tx) =
                                                        apply_edit_slot.lock().await.take()
                                                    {
                                                        let _ = tx.send(edit);
                                                    }
                                                    let resp = serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": id_val,
                                                        "result": { "applied": true }
                                                    });
                                                    let _ =
                                                        Self::write_frame_raw(&stdin_writer, &resp)
                                                            .await;
                                                }
                                                "workspace/workspaceFolders" => {
                                                    let resp = serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": id_val,
                                                        "result": null
                                                    });
                                                    let _ =
                                                        Self::write_frame_raw(&stdin_writer, &resp)
                                                            .await;
                                                }
                                                other => {
                                                    // Unknown server request: refuse it instead of
                                                    // leaving the server blocked on the answer.
                                                    tracing::debug!(
                                                        method = other,
                                                        "unsupported server request refused"
                                                    );
                                                    let resp = serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": id_val,
                                                        "error": { "code": -32601, "message": format!("{other} is not supported by prod-code") }
                                                    });
                                                    let _ =
                                                        Self::write_frame_raw(&stdin_writer, &resp)
                                                            .await;
                                                }
                                            }
                                        }
                                    }

                                    if val.get("method").and_then(|m| m.as_str())
                                        == Some("textDocument/publishDiagnostics")
                                        && let Some(uri) = val
                                            .get("params")
                                            .and_then(|p| p.get("uri"))
                                            .and_then(|u| u.as_str())
                                    {
                                        let items = val
                                            .get("params")
                                            .and_then(|p| p.get("diagnostics"))
                                            .and_then(|d| d.as_array())
                                            .cloned()
                                            .unwrap_or_default();
                                        diagnostics_writer
                                            .write()
                                            .await
                                            .insert(uri.to_string(), items);
                                    }
                                    let _ = bcast_tx_clone.send(json_str);
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            is_alive_clone.store(false, Ordering::Relaxed);
            tracing::info!("Generic LSP reader loop finished");
        });

        let engine = Self {
            workspace_root: workspace_root.to_path_buf(),
            config,
            stdin: stdin_arc,
            next_req_id: AtomicU64::new(1),
            pending_requests,
            capabilities: Arc::new(RwLock::new(None)),
            broadcast_tx: bcast_tx,
            last_activity,
            diagnostics,
            apply_edit_waiter,
            is_alive,
            _child: Arc::new(Mutex::new(child)),
        };

        // Initialize LSP server
        engine.initialize().await?;

        Ok(engine)
    }

    /// Helper to write an LSP Content-Length frame.
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

    /// Perform the standard LSP initialize handshake.
    pub async fn initialize(&self) -> Result<serde_json::Value> {
        let ws_str = self.workspace_root.to_string_lossy().to_string();
        let ws_name = self
            .workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("generic-workspace");

        let mut init_params = serde_json::json!({
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

        if let Some(options) = &self.config.initialization_options {
            init_params["initializationOptions"] = options.clone();
        }
        let resp = self.send_request("initialize", init_params).await?;

        if let Some(caps) = resp.get("result").and_then(|r| r.get("capabilities")) {
            let mut guard = self.capabilities.write().await;
            *guard = Some(caps.clone());
        }

        self.send_notification("initialized", serde_json::json!({}))
            .await?;

        Ok(resp)
    }

    /// Send a request and await its response.
    /// Runs `workspace/executeCommand` and returns the WorkspaceEdit the server pushes back
    /// through `workspace/applyEdit` while doing so (clangd and others deliver refactorings
    /// this way), or `None` when the command finished without an edit.
    pub async fn execute_command_capturing_edit(
        &self,
        command: serde_json::Value,
    ) -> Result<Option<serde_json::Value>> {
        let (tx, rx) = oneshot::channel();
        *self.apply_edit_waiter.lock().await = Some(tx);
        let params = serde_json::json!({
            "command": command.get("command").cloned().unwrap_or(serde_json::Value::Null),
            "arguments": command.get("arguments").cloned().unwrap_or(serde_json::json!([])),
        });
        let response = self.send_request("workspace/executeCommand", params).await;
        let edit = match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(edit)) if !edit.is_null() => Some(edit),
            _ => None,
        };
        *self.apply_edit_waiter.lock().await = None;
        let response = response?;
        if let Some(err) = response.get("error") {
            anyhow::bail!(
                "{}",
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("command failed")
            );
        }
        Ok(edit)
    }

    /// Whether the server advertises pull diagnostics (`textDocument/diagnostic`).
    pub async fn has_pull_diagnostics(&self) -> bool {
        self.capabilities
            .read()
            .await
            .as_ref()
            .and_then(|c| c.get("diagnosticProvider"))
            .is_some_and(|d| !d.is_null())
    }

    /// Whether the server has published diagnostics for `uri` at least once.
    pub async fn diagnostics_published(&self, uri: &str) -> bool {
        self.diagnostics.read().await.contains_key(uri)
    }

    /// The diagnostics the server last published for `uri` (empty when none).
    pub async fn diagnostics_for(&self, uri: &str) -> Vec<serde_json::Value> {
        self.diagnostics
            .read()
            .await
            .get(uri)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        if !self.is_alive.load(Ordering::Relaxed) {
            anyhow::bail!("Language server process has exited");
        }

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

        match tokio::time::timeout(self.config.request_timeout, rx).await {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(_)) => anyhow::bail!("LSP request channel dropped unexpectedly"),
            Err(_) => {
                let mut pending = self.pending_requests.lock().await;
                pending.remove(&req_id);
                anyhow::bail!("Timeout waiting for response to '{method}'");
            }
        }
    }

    /// Send a notification to the language server.
    pub async fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        Self::write_frame_raw(&self.stdin, &payload).await
    }

    /// Check if the process is currently running and healthy.
    pub fn is_alive(&self) -> bool {
        self.is_alive.load(Ordering::Relaxed)
    }

    /// Elapsed time since the last active message.
    pub async fn idle_duration(&self) -> Duration {
        self.last_activity.read().await.elapsed()
    }

    /// Subscribe to background broadcast notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.broadcast_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generic_config_defaults() {
        let py_config = GenericLspConfig::for_python();
        assert!(!py_config.command.is_empty());

        let ts_config = GenericLspConfig::for_typescript();
        assert!(!ts_config.command.is_empty());
    }

    #[test]
    fn test_which_bin_discovery() {
        assert!(which_bin("cargo").is_ok());
        assert!(which_bin("nonexistent_binary_xyz_123").is_err());
    }
}
