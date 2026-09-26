//! Pluggable Generic LSP engine for external language servers (Pyright, Ruff, TypeScript, etc.).
//!
//! Provides supervised process lifecycle, automatic framing, request/response routing,
//! health monitoring, and idle shutdown management.

use anyhow::{Context, Result};
use prod_code_protocol::readiness::{
    BUSY_MEMBER, Busy, INDEX_WAIT, Readiness, ReadySignal, needs_index, pyright_found_sources,
};
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
    /// How the server tells that it has loaded and indexed its project, so that questions
    /// answered from its index wait for it instead of getting nothing or a part (#391).
    pub ready: ReadySignal,
    /// How long such a question waits for the server at most before it is asked anyway, with
    /// a note of how far the server got.
    pub index_wait: Duration,
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
            ready: ReadySignal::Unknown,
            index_wait: INDEX_WAIT,
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
        // basedpyright and pyright report no progress; they log `Found N source files` once
        // their program is set up, and hold a question from then on.
        let ready = if cmd.contains("pyright") {
            ReadySignal::Log(pyright_found_sources)
        } else {
            ReadySignal::Unknown
        };

        Self {
            command: cmd,
            args,
            env: HashMap::new(),
            working_dir: None,
            initialization_options: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            ready,
            index_wait: INDEX_WAIT,
        }
    }

    /// Create a configuration for C/C++ (clangd). A `compile_commands.json` at the workspace
    /// root or under `build/` gives clangd the real flags. Without `--use-dirty-headers` clangd
    /// parses an included header from disk even when its proposed text is open, so a check of a
    /// header edit together with its sources judged the sources against the old header (#292).
    pub fn for_cpp() -> Self {
        Self {
            command: "clangd".to_string(),
            args: vec![
                "--background-index".to_string(),
                "--header-insertion=never".to_string(),
                "--use-dirty-headers".to_string(),
                "--log=error".to_string(),
                "--compile-commands-dir=build".to_string(),
            ],
            env: HashMap::new(),
            working_dir: None,
            initialization_options: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            // `backgroundIndexProgress`: begun at once, ended when the index is complete.
            ready: ReadySignal::Progress,
            index_wait: INDEX_WAIT,
        }
    }

    /// The clangd that only validation sessions reach: [`Self::for_cpp`] without the
    /// background index, since a validation session asks only for the diagnostics of the texts
    /// it opens. The proposed texts stay out of the main server, where clangd kept a closed
    /// document in its index as last built and went on answering `references` from it (#293).
    pub fn for_cpp_validation() -> Self {
        let mut config = Self::for_cpp();
        config.args.retain(|a| !a.starts_with("--background-index"));
        config.args.push("--background-index=false".to_string());
        // Without a background index there is nothing to wait for.
        config.ready = ReadySignal::HoldsQuestions;
        config
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
            // It reports reloading the package as progress.
            ready: ReadySignal::Progress,
            index_wait: INDEX_WAIT,
        }
    }

    /// Create a standard configuration for TypeScript / JavaScript language servers.
    pub fn for_typescript() -> Self {
        // TypeScript 7 (native) ships its own LSP: `tsc --lsp --stdio` from the platform
        // package. It needs no tsserver and no Node at all, so it wins when present.
        let native = native_typescript_lsp();
        // The native server holds a question until its project is loaded; what the others do
        // is not known.
        let ready = if native.is_some() {
            ReadySignal::HoldsQuestions
        } else {
            ReadySignal::Unknown
        };
        let (cmd, args) = if let Some(native) = native {
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
            ready,
            index_wait: INDEX_WAIT,
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
    diagnostics: Arc<RwLock<HashMap<String, Published>>>,
    /// The text last sent per document URI (`didOpen`, `didChange`), which a publication has
    /// to cover before it answers for the document (#293).
    sent: RwLock<HashMap<String, Sent>>,
    /// Whether the server has published with a document version, as clangd does: then a
    /// publication without one (clangd's, for a document just closed) describes no text sent.
    versioned: Arc<AtomicBool>,
    /// Whether the server answered a diagnostic pull with "method not found" (clangd does).
    pull_unsupported: AtomicBool,
    /// Receives the `workspace/applyEdit` a server sends while a command runs.
    apply_edit_waiter: Arc<Mutex<Option<oneshot::Sender<serde_json::Value>>>>,
    is_alive: Arc<AtomicBool>,
    /// What the server has said about loading and indexing its project (#391).
    readiness: Arc<Readiness>,
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
        let diagnostics: Arc<RwLock<HashMap<String, Published>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let diagnostics_writer = diagnostics.clone();
        let versioned = Arc::new(AtomicBool::new(false));
        let versioned_writer = Arc::clone(&versioned);
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

        let readiness = Arc::new(Readiness::new(config.ready));
        let readiness_reader = Arc::clone(&readiness);

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
                                    readiness_reader.on_message(&val);
                                    if let Some(id_val) = val.get("id") {
                                        // Only an answer is ours: a request from the server
                                        // (`window/workDoneProgress/create`) numbers its own ids
                                        // from 1 too, and taken for the answer to ours it left
                                        // the server waiting and our question empty (#391).
                                        if val.get("method").is_none()
                                            && let Some(id) = id_val.as_u64()
                                        {
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
                                        let version =
                                            val.pointer("/params/version").and_then(|v| v.as_i64());
                                        if version.is_some() {
                                            versioned_writer.store(true, Ordering::Relaxed);
                                        }
                                        diagnostics_writer.write().await.insert(
                                            uri.to_string(),
                                            Published {
                                                version,
                                                at: Instant::now(),
                                                items,
                                            },
                                        );
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
            // No answer is coming for a request still waiting: dropping its sender ends the
            // wait now, not at the request timeout (#355).
            pending_clone.lock().await.clear();
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
            sent: RwLock::new(HashMap::new()),
            versioned,
            pull_unsupported: AtomicBool::new(false),
            apply_edit_waiter,
            is_alive,
            readiness,
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
                // Servers report loading and indexing as progress only to a client that says
                // it takes it (#391).
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
        self.readiness.started();

        Ok(resp)
    }

    /// Whether the server's readiness is known, so that its index answers are final once it
    /// has said it is ready (#391).
    pub fn readiness_known(&self) -> bool {
        self.readiness.known()
    }

    /// The loading or indexing the server is still doing, if any.
    pub fn busy(&self) -> Option<Busy> {
        self.readiness.busy()
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

    /// The document's diagnostics as the server computes them now, asked with a pull
    /// (`textDocument/diagnostic`), or `None` when the server does not answer one. It is asked
    /// whether or not it advertised the method: sourcekit-lsp answers a pull without
    /// advertising it, and what it publishes first for a document is an empty list, ahead of
    /// the check that finds the errors (#293). A server that does not know the method (clangd)
    /// is not asked again.
    pub async fn pull_diagnostics(&self, uri: &str) -> Option<Vec<serde_json::Value>> {
        if self.pull_unsupported.load(Ordering::Relaxed) {
            return None;
        }
        let answer = self
            .send_request(
                "textDocument/diagnostic",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await
            .ok()?;
        if answer.pointer("/error/code").and_then(|c| c.as_i64()) == Some(METHOD_NOT_FOUND) {
            self.pull_unsupported.store(true, Ordering::Relaxed);
            return None;
        }
        answer.pointer("/result/items")?.as_array().cloned()
    }

    /// Opens `text` as the file at `path` until the server reports an error on its 0-based
    /// `line`, closes it again, and says whether that happened within `timeout`. `text` is a
    /// real file of the project with a line appended that only a type check can fault, so a
    /// server that faults it checks files with the project's real build settings. sourcekit-lsp
    /// loads a package's settings in the background after it starts; until then it checks with
    /// fallback settings that report syntax errors only, and a check answered in that window
    /// found no errors at all (#295).
    pub async fn wait_for_semantic_check(
        &self,
        path: &Path,
        language_id: &str,
        text: &str,
        line: u64,
        timeout: Duration,
    ) -> bool {
        let Ok(uri) = url::Url::from_file_path(path) else {
            return false;
        };
        let uri = uri.to_string();
        let open = serde_json::json!({ "textDocument": {
            "uri": uri, "languageId": language_id, "version": 1, "text": text
        }});
        if self
            .send_notification("textDocument/didOpen", open)
            .await
            .is_err()
        {
            return false;
        }
        let started = Instant::now();
        let mut faulted = false;
        while !faulted && started.elapsed() < timeout {
            let items = match self.pull_diagnostics(&uri).await {
                Some(items) => items,
                None => self.current_diagnostics_for(&uri, SEMANTIC_POLL).await,
            };
            faulted = items.iter().any(|d| {
                d.get("severity").and_then(|s| s.as_u64()) == Some(1)
                    && d.pointer("/range/start/line").and_then(|l| l.as_u64()) == Some(line)
            });
            if !faulted {
                tokio::time::sleep(SEMANTIC_POLL).await;
            }
        }
        let close = serde_json::json!({ "textDocument": { "uri": uri } });
        let _ = self.send_notification("textDocument/didClose", close).await;
        faulted
    }

    /// Whether the server has published diagnostics for `uri` at least once.
    pub async fn diagnostics_published(&self, uri: &str) -> bool {
        self.diagnostics.read().await.contains_key(uri)
    }

    /// The diagnostics the server last published for `uri` (empty when none), whichever text
    /// they were for.
    pub async fn diagnostics_for(&self, uri: &str) -> Vec<serde_json::Value> {
        self.diagnostics
            .read()
            .await
            .get(uri)
            .map(|p| p.items.clone())
            .unwrap_or_default()
    }

    /// The diagnostics for the text last sent for `uri`. A server that pushes diagnostics
    /// publishes for each text it builds, and a publication for the text before the last
    /// change may still arrive after that change was sent; answering with it reports the old
    /// text's errors as the new one's (#293). This waits up to `wait` for a publication that
    /// covers the last text sent, and up to [`FIRST_PUBLICATION_WAIT`] for a first one when
    /// nothing was sent for the document; past that it answers with what was last published.
    pub async fn current_diagnostics_for(
        &self,
        uri: &str,
        wait: Duration,
    ) -> Vec<serde_json::Value> {
        let started = Instant::now();
        loop {
            let known = {
                let sent = self.sent.read().await;
                let published = self.diagnostics.read().await;
                if let Some(p) = published.get(uri)
                    && covers(p, sent.get(uri), self.versioned.load(Ordering::Relaxed))
                {
                    return p.items.clone();
                }
                sent.contains_key(uri)
            };
            let limit = if known {
                wait
            } else {
                wait.min(FIRST_PUBLICATION_WAIT)
            };
            if started.elapsed() >= limit {
                if known {
                    tracing::warn!(
                        uri,
                        waited_ms = started.elapsed().as_millis() as u64,
                        "no diagnostics published for the text last sent; answering with the last ones"
                    );
                }
                return self.diagnostics_for(uri).await;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        if !self.is_alive.load(Ordering::Relaxed) {
            anyhow::bail!("Language server process has exited");
        }
        // A question answered from the index waits until the server has built it; one still
        // not built when the wait ends is answered with a note of how far it got (#391).
        let busy = if needs_index(method) {
            self.readiness.wait(self.config.index_wait).await
        } else {
            None
        };

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
            Ok(Ok(mut val)) => {
                if let Some(busy) = busy {
                    val[BUSY_MEMBER] = serde_json::to_value(busy)?;
                }
                Ok(val)
            }
            Ok(Err(_)) => {
                anyhow::bail!("Language server process has exited while answering '{method}'")
            }
            Err(_) => {
                let mut pending = self.pending_requests.lock().await;
                pending.remove(&req_id);
                anyhow::bail!("Timeout waiting for response to '{method}'");
            }
        }
    }

    /// Send a notification to the language server.
    pub async fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        // Recorded before the text is on its way, so no publication for it can come first.
        if let Some(uri) = params.pointer("/textDocument/uri").and_then(|u| u.as_str()) {
            // A publication from before an open or a close describes a text that is gone:
            // another session's, which numbered its versions from 1 as this one does.
            if matches!(method, "textDocument/didOpen" | "textDocument/didClose") {
                self.diagnostics.write().await.remove(uri);
            }
            match method {
                "textDocument/didOpen" | "textDocument/didChange" => {
                    let version = params
                        .pointer("/textDocument/version")
                        .and_then(|v| v.as_i64());
                    self.sent.write().await.insert(
                        uri.to_string(),
                        Sent {
                            version,
                            at: Instant::now(),
                        },
                    );
                }
                "textDocument/didClose" => {
                    self.sent.write().await.remove(uri);
                }
                _ => {}
            }
        }
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        Self::write_frame_raw(&self.stdin, &payload).await
    }

    /// Check if the process is currently running and healthy.
    /// Whether the server process is still running: false once its output has ended, as it
    /// does when the server exits or crashes. The gateway loads a workspace afresh when its
    /// server has exited (#355).
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

/// JSON-RPC's code for a method the server does not know.
const METHOD_NOT_FOUND: i64 = -32601;

/// How often [`GenericLspEngine::wait_for_semantic_check`] asks again.
const SEMANTIC_POLL: Duration = Duration::from_millis(300);

/// How long [`GenericLspEngine::current_diagnostics_for`] waits for the first publication for a
/// document no text was sent for: one the server opened by itself, or one it will never
/// publish for.
pub const FIRST_PUBLICATION_WAIT: Duration = Duration::from_secs(3);

/// What a server last published for one document.
#[derive(Debug, Clone)]
struct Published {
    /// The document version the publication was for, when the server says (clangd does).
    version: Option<i64>,
    /// When it arrived.
    at: Instant,
    items: Vec<serde_json::Value>,
}

/// The text last sent for one document.
#[derive(Debug, Clone)]
struct Sent {
    version: Option<i64>,
    at: Instant,
}

/// Whether a publication covers the text last sent for its document: one for that version or
/// a later one, or, when either side has no version, one that arrived after the text was sent.
/// From a server that numbers its publications (`versioned`), one without a number never
/// covers a numbered text. With nothing sent, any publication does.
fn covers(published: &Published, sent: Option<&Sent>, versioned: bool) -> bool {
    match sent {
        None => true,
        Some(sent) => match (published.version, sent.version) {
            (Some(p), Some(s)) => p >= s,
            (None, Some(_)) if versioned => false,
            _ => published.at >= sent.at,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_publication_answers_for_the_last_text_only_when_it_covers_it() {
        let now = Instant::now();
        let later = now + Duration::from_millis(5);
        let published = |version, at| Published {
            version,
            at,
            items: Vec::new(),
        };
        let sent = |version, at| Sent { version, at };
        for versioned in [false, true] {
            assert!(
                covers(&published(Some(1), now), None, versioned),
                "nothing sent"
            );
            assert!(
                !covers(
                    &published(Some(1), later),
                    Some(&sent(Some(2), now)),
                    versioned
                ),
                "the text before the change, even when it arrives after it"
            );
            assert!(covers(
                &published(Some(2), later),
                Some(&sent(Some(2), now)),
                versioned
            ));
            assert!(
                covers(
                    &published(Some(3), later),
                    Some(&sent(Some(2), now)),
                    versioned
                ),
                "a later version covers an earlier one"
            );
            assert!(
                !covers(
                    &published(None, now),
                    Some(&sent(Some(2), later)),
                    versioned
                ),
                "without a version, one from before the text was sent does not"
            );
            assert!(covers(
                &published(Some(1), later),
                Some(&sent(None, now)),
                versioned
            ));
        }
        assert!(
            covers(&published(None, later), Some(&sent(Some(2), now)), false),
            "from a server without versions, one that arrived after the text covers it"
        );
        assert!(
            !covers(&published(None, later), Some(&sent(Some(2), now)), true),
            "from clangd, a publication without a version is a closed document's"
        );
    }

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
