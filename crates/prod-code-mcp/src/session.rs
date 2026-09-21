//! A persistent gateway session for batches of LSP queries: one connection, one pre-flight
//! sync, one handshake and one `initialize`, then any number of requests (documents are
//! opened once). Batch features (impact analysis, dead-code scans) use this instead of a
//! connection per query.

use crate::sync::{WorkspaceIdentity, engine_project, push_workspace_sync, workspace_identity};
use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use url::Url;

pub struct LspSession {
    framed: Framed<TcpStream, ProdCodeCodec>,
    root: PathBuf,
    opened: HashSet<String>,
    next_id: i64,
    /// The engine the gateway chose for this session.
    pub engine: String,
}

impl LspSession {
    /// Opens a session on `remote` for the checkout at `root`. `hint` selects a nested
    /// project (any path inside it); the root project otherwise.
    pub async fn open(remote: SocketAddr, root: &Path, hint: Option<&Path>) -> Result<Self> {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let root_str = root.to_string_lossy().to_string();
        let identity: WorkspaceIdentity = workspace_identity(&root);
        let stream = TcpStream::connect(remote)
            .await
            .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
        let _ = stream.set_nodelay(true);
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        push_workspace_sync(&mut framed, &root, &identity, None)
            .await
            .context("pre-flight workspace sync failed")?;
        let (engine_subpath, _) = engine_project(&root, hint.unwrap_or(&root));
        framed
            .send(WireMessage::HandshakeRequest(HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                client_name: "prod-code-batch".to_string(),
                client_pid: std::process::id(),
                auth_token: None,
                client_workspace_root: root_str.clone(),
                preferred_engine: None,
                base_workspace_name: Some(identity.name.clone()),
                engine_subpath,
                client_agent: Some(prod_code_protocol::detect_client_agent()),
                client_host: Some(prod_code_protocol::client_host()),
            }))
            .await?;
        let handshake = match framed.next().await {
            Some(Ok(WireMessage::HandshakeResponse(resp))) => resp,
            other => anyhow::bail!("unexpected handshake response: {other:?}"),
        };
        let folder_name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_string();
        let mut session = Self {
            framed,
            root,
            opened: HashSet::new(),
            next_id: 1,
            engine: handshake.detected_engine,
        };
        let init = serde_json::json!({
            "processId": null,
            "rootUri": format!("file://{root_str}"),
            "workspaceFolders": [{ "name": folder_name, "uri": format!("file://{root_str}") }],
            "capabilities": {
                "workspace": { "workspaceFolders": true, "configuration": true },
                "textDocument": {
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "definition": { "linkSupport": true },
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "references": {}
                }
            }
        });
        session.request("initialize", init).await?;
        session.notify("initialized", serde_json::json!({})).await?;
        Ok(session)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    async fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<()> {
        let msg = serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.framed
            .send(WireMessage::LspPayload(msg.to_string()))
            .await?;
        Ok(())
    }

    pub(crate) async fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg =
            serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.framed
            .send(WireMessage::LspPayload(msg.to_string()))
            .await?;
        // A structural rewrite searches the workspace with type inference and legitimately
        // takes minutes; everything else is an interactive query and should not.
        let budget = if method == "prodCode/structuralReplace" {
            std::time::Duration::from_secs(900)
        } else {
            std::time::Duration::from_secs(60)
        };
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("timeout waiting for {method}");
            }
            match tokio::time::timeout(remaining, self.framed.next()).await {
                Ok(Some(Ok(WireMessage::LspPayload(json)))) => {
                    let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) else {
                        continue;
                    };
                    if val.get("id").and_then(|i| i.as_i64()) != Some(id) {
                        continue;
                    }
                    if let Some(err) = val.get("error") {
                        let message = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error");
                        anyhow::bail!("{method} failed: {message}");
                    }
                    return Ok(val
                        .get("result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null));
                }
                Ok(Some(Ok(WireMessage::Ping))) => {
                    let _ = self.framed.send(WireMessage::Pong).await;
                }
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(e))) => anyhow::bail!("frame decode error: {e}"),
                Ok(None) => anyhow::bail!("gateway closed the session"),
                Err(_) => anyhow::bail!("timeout waiting for {method}"),
            }
        }
    }

    /// Opens `file` (once) and runs `method` with `params` on it.
    pub async fn query(
        &mut self,
        file: &Path,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            self.root.join(file)
        };
        let uri = Url::from_file_path(&abs)
            .map_err(|_| anyhow!("invalid file path {}", abs.display()))?
            .to_string();
        // A directory stands for a workspace-level query (workspace/symbol): nothing to open.
        if !self.opened.contains(&uri) && !abs.is_dir() {
            let text = tokio::fs::read_to_string(&abs)
                .await
                .with_context(|| format!("failed to read {}", abs.display()))?;
            self.notify(
                "textDocument/didOpen",
                serde_json::json!({ "textDocument": {
                    "uri": uri,
                    "languageId": crate::lang::language_id_for_path(&abs),
                    "version": 1,
                    "text": text,
                }}),
            )
            .await?;
            self.opened.insert(uri.clone());
        }
        self.request(method, params).await
    }

    /// Runs `method` on `file` with `text` as its content instead of what is on disk: the
    /// document is opened (or changed) with the proposed text, so the server analyses the
    /// edit without anything being written.
    pub async fn query_with_text(
        &mut self,
        file: &Path,
        text: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.open_text(file, text).await?;
        self.request(method, params).await
    }

    /// Makes `text` the content of `file` in this session (didOpen, or didChange when the
    /// document is already open) without querying, so several proposed files can be in
    /// place before diagnostics are pulled for any of them. Returns the document's URI.
    pub async fn open_text(&mut self, file: &Path, text: &str) -> Result<String> {
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            self.root.join(file)
        };
        let uri = Url::from_file_path(&abs)
            .map_err(|_| anyhow!("invalid file path {}", abs.display()))?
            .to_string();
        if self.opened.contains(&uri) {
            self.notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": { "uri": uri, "version": self.next_id },
                    "contentChanges": [ { "text": text } ]
                }),
            )
            .await?;
        } else {
            self.notify(
                "textDocument/didOpen",
                serde_json::json!({ "textDocument": {
                    "uri": uri,
                    "languageId": crate::lang::language_id_for_path(&abs),
                    "version": 1,
                    "text": text,
                }}),
            )
            .await?;
            self.opened.insert(uri.clone());
        }
        Ok(uri)
    }

    /// The `file://` URI the session uses for `file`.
    pub fn uri_for(&self, file: &Path) -> Result<String> {
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            self.root.join(file)
        };
        Ok(Url::from_file_path(&abs)
            .map_err(|_| anyhow!("invalid file path {}", abs.display()))?
            .to_string())
    }

    /// Pushes the checkout's changes since the last sync over this same connection and
    /// brings the documents the session holds open in line with them (`didChange` for
    /// rewritten files, `didClose` for deleted ones), so a long-lived session sees every
    /// local edit exactly like a fresh one would.
    pub async fn refresh(&mut self) -> Result<()> {
        let identity = workspace_identity(&self.root);
        let outcome = push_workspace_sync(&mut self.framed, &self.root, &identity, None)
            .await
            .context("workspace sync on the open session failed")?;
        for rel in outcome.changed_paths {
            let abs = self.root.join(&rel);
            let Ok(uri) = Url::from_file_path(&abs).map(|u| u.to_string()) else {
                continue;
            };
            if !self.opened.contains(&uri) {
                continue;
            }
            match tokio::fs::read_to_string(&abs).await {
                Ok(text) => {
                    let version = self.next_id;
                    self.next_id += 1;
                    self.notify(
                        "textDocument/didChange",
                        serde_json::json!({
                            "textDocument": { "uri": uri, "version": version },
                            "contentChanges": [ { "text": text } ]
                        }),
                    )
                    .await?;
                }
                Err(_) => {
                    self.notify(
                        "textDocument/didClose",
                        serde_json::json!({ "textDocument": { "uri": uri } }),
                    )
                    .await?;
                    self.opened.remove(&uri);
                }
            }
        }
        Ok(())
    }

    /// Ends the session cleanly.
    pub async fn close(mut self) {
        for uri in std::mem::take(&mut self.opened) {
            let _ = self
                .notify(
                    "textDocument/didClose",
                    serde_json::json!({ "textDocument": { "uri": uri } }),
                )
                .await;
        }
        let _ = self
            .framed
            .send(WireMessage::Disconnect {
                reason: "batch finished".to_string(),
            })
            .await;
    }
}

/// Long-lived sessions of this process, one per (gateway, checkout, nested project): the
/// MCP server keeps them across tool calls so a query costs one round trip instead of a
/// connection, a sync and a handshake each time.
fn pool() -> &'static tokio::sync::Mutex<HashMap<String, LspSession>> {
    static POOL: OnceLock<tokio::sync::Mutex<HashMap<String, LspSession>>> = OnceLock::new();
    POOL.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

fn is_connection_error(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}").to_ascii_lowercase();
    text.contains("closed")
        || text.contains("timeout")
        || text.contains("timed out")
        || text.contains("broken pipe")
        || text.contains("reset")
        || text.contains("decode")
        || text.contains("connection")
}

/// Runs one query on the pooled session for `root` (opening it on first use): local
/// changes are pushed first when the watcher saw any, and a session whose connection died
/// (gateway restart) is replaced and the query retried once.
pub async fn pooled_query(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let (subpath, _) = engine_project(&root, file);
    let key = format!(
        "{remote}|{}|{}",
        root.display(),
        subpath.unwrap_or_default()
    );
    let mut sessions = pool().lock().await;
    for attempt in 0..2 {
        if !sessions.contains_key(&key) {
            let session = LspSession::open(remote, &root, Some(file)).await?;
            crate::watch::mark_synced(&root, crate::watch::current_generation(&root));
            sessions.insert(key.clone(), session);
        }
        let session = sessions.get_mut(&key).expect("just inserted");
        let generation = crate::watch::current_generation(&root);
        if crate::watch::sync_due(&root, generation) {
            match session.refresh().await {
                Ok(()) => crate::watch::mark_synced(&root, generation),
                Err(err) if is_connection_error(&err) && attempt == 0 => {
                    sessions.remove(&key);
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
        match session.query(file, method, params.clone()).await {
            Ok(value) => return Ok(value),
            Err(err) if is_connection_error(&err) && attempt == 0 => {
                sessions.remove(&key);
                continue;
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("two attempts always return")
}
