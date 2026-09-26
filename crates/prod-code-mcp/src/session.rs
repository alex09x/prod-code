//! A persistent gateway session for batches of LSP queries: one connection, one pre-flight
//! sync, one handshake and one `initialize`, then any number of requests (documents are
//! opened once). Batch features (impact analysis, dead-code scans) use this instead of a
//! connection per query.

use crate::sync::{
    WorkspaceIdentity, engine_project, gateway_node, push_workspace_sync, resend_lost_files,
    workspace_identity,
};
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
    /// The documents open in the server, by URI: the file, and a hash of the disk text last
    /// sent for it (`None` for a proposed text, which is not the file's).
    opened: HashMap<String, (PathBuf, Option<u64>)>,
    next_id: i64,
    /// The engine the gateway chose for this session.
    pub engine: String,
    /// When the gateway loaded that engine; `None` when it does not say (#381).
    engine_loaded: Option<std::time::Instant>,
}

/// How long after its engine was loaded an empty `workspace/symbol` answer may still be early:
/// a language server indexes after it starts (#381).
pub const INDEXING_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// A hash of a document's text, to tell whether the file still holds what was sent.
fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// How long a request may take before the session gives up on it. A structural rewrite searches
/// the workspace with type inference and legitimately takes minutes. A file's diagnostics are a
/// full check of it: 85 s cold for a 4,246-line file on an aarch64 build node, where 60 s made
/// the client give up and send the same work again (#237). Everything else is an interactive
/// query and should not take long.
fn budget_for(method: &str) -> std::time::Duration {
    std::time::Duration::from_secs(match method {
        "prodCode/structuralReplace" => 900,
        "textDocument/diagnostic" => 300,
        _ => 60,
    })
}

impl LspSession {
    /// How long ago the gateway loaded this session's engine; `None` when it does not say
    /// (#381).
    pub fn engine_age(&self) -> Option<std::time::Duration> {
        self.engine_loaded.map(|at| at.elapsed())
    }

    /// Opens a session on `remote` for the checkout at `root`. `hint` selects a nested
    /// project (any path inside it); the root project otherwise.
    pub async fn open(remote: SocketAddr, root: &Path, hint: Option<&Path>) -> Result<Self> {
        Self::open_with_purpose(remote, root, hint, None).await
    }

    /// A session that opens proposed texts only to validate them. The gateway serves it from a
    /// second engine for the workspace, so an overlay that changes what a widely imported file
    /// declares — and the revert when the session closes — never invalidates the main engine's
    /// work, and the next ordinary query does not pay for it (#73).
    pub async fn open_for_validation(
        remote: SocketAddr,
        root: &Path,
        hint: Option<&Path>,
    ) -> Result<Self> {
        Self::open_with_purpose(
            remote,
            root,
            hint,
            Some(prod_code_protocol::PURPOSE_VALIDATION),
        )
        .await
    }

    async fn open_with_purpose(
        remote: SocketAddr,
        root: &Path,
        hint: Option<&Path>,
        purpose: Option<&str>,
    ) -> Result<Self> {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let root_str = root.to_string_lossy().to_string();
        let identity: WorkspaceIdentity = workspace_identity(&root);
        let stream = prod_code_protocol::transport::connect(remote)
            .await
            .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        push_workspace_sync(&mut framed, &root, &identity, None)
            .await
            .context("pre-flight workspace sync failed")?;
        let (engine_subpath, engine) = engine_project(&root, hint.unwrap_or(&root));
        // A nested project's engine is named, so a directory with no manifest of its own (a
        // loose script's) is served by its language, not by detection there (#247).
        let preferred_engine = engine_subpath.as_ref().and(engine).map(str::to_string);
        framed
            .send(WireMessage::HandshakeRequest(HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                client_name: "prod-code-batch".to_string(),
                client_pid: std::process::id(),
                auth_token: None,
                client_workspace_root: root_str.clone(),
                preferred_engine,
                base_workspace_name: Some(identity.name.clone()),
                engine_subpath,
                client_agent: Some(prod_code_protocol::detect_client_agent()),
                client_host: Some(prod_code_protocol::client_host()),
                purpose: purpose.map(str::to_string),
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
            opened: HashMap::new(),
            next_id: 1,
            engine: handshake.detected_engine,
            engine_loaded: handshake.engine_age_ms.and_then(|ms| {
                std::time::Instant::now().checked_sub(std::time::Duration::from_millis(ms))
            }),
        };
        // Files the gateway lost from its copy (#262) that the sync above did not carry: the
        // next sync sends them again.
        resend_lost_files(
            &session.root,
            &gateway_node(&session.framed),
            &handshake.stale_paths,
        );
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
        let budget = budget_for(method);
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
        // Neither does a file outside the checkout that is not on this machine, such as a
        // dependency's source in the node's cargo registry: the node's analyzer already has
        // it, and reading it here would fail (#271).
        let only_on_the_node = !abs.starts_with(&self.root) && !abs.exists();
        if !self.opened.contains_key(&uri) && !abs.is_dir() && !only_on_the_node {
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
            self.opened
                .insert(uri.clone(), (abs.clone(), Some(text_hash(&text))));
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
        if self.opened.contains_key(&uri) {
            self.opened.insert(uri.clone(), (abs.clone(), None));
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
            self.opened.insert(uri.clone(), (abs.clone(), None));
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
    /// brings the documents the session holds open in line with the files (`didChange` for a
    /// rewritten file, `didClose` for a deleted one), so a long-lived session sees every local
    /// edit exactly like a fresh one would. That is every file this sync pushed, and every file
    /// opened from disk whose text is no longer what was sent: another client (the CLI, an
    /// `exec` whose formatter's output came back, another agent) may have pushed it to the node
    /// already, and this sync then pushes nothing for it (#360).
    pub async fn refresh(&mut self) -> Result<()> {
        let identity = workspace_identity(&self.root);
        let outcome = push_workspace_sync(&mut self.framed, &self.root, &identity, None)
            .await
            .context("workspace sync on the open session failed")?;
        let pushed: HashSet<String> = outcome
            .changed_paths
            .iter()
            .filter_map(|rel| Url::from_file_path(self.root.join(rel)).ok())
            .map(|uri| uri.to_string())
            .collect();
        let open: Vec<(String, PathBuf, Option<u64>)> = self
            .opened
            .iter()
            .map(|(uri, (path, sent))| (uri.clone(), path.clone(), *sent))
            .collect();
        for (uri, path, sent) in open {
            let was_pushed = pushed.contains(&uri);
            if !was_pushed && sent.is_none() {
                continue;
            }
            match tokio::fs::read_to_string(&path).await {
                Ok(text) => {
                    let hash = text_hash(&text);
                    if !was_pushed && sent == Some(hash) {
                        continue;
                    }
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
                    self.opened.insert(uri, (path, Some(hash)));
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
        for uri in std::mem::take(&mut self.opened).into_keys() {
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
        // The gateway's language server crashed: a new session gets a new one (#355).
        || text.contains("has exited")
}

/// The checkout a query about `file` is asked in, and the key of its pooled session: one per
/// node, checkout and nested project.
fn pool_key(remote: SocketAddr, root: &Path, file: &Path) -> (PathBuf, String) {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    // A local file of another checkout, such as a clone next to this one, is asked about in
    // that checkout's own session: this one's analyzer never loaded it, and a server of another
    // language only fails on it (#353).
    let root = crate::sync::other_checkout(&root, file).unwrap_or(root);
    let (subpath, _) = engine_project(&root, file);
    let key = format!(
        "{remote}|{}|{}",
        root.display(),
        subpath.unwrap_or_default()
    );
    (root, key)
}

/// [`LspSession::engine_age`] of the pooled session that answers about `file` in the checkout
/// at `root`; `None` when there is none yet or its gateway does not say (#381).
pub async fn pooled_engine_age(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
) -> Option<std::time::Duration> {
    let (_, key) = pool_key(remote, root, file);
    pool()
        .lock()
        .await
        .get(&key)
        .and_then(LspSession::engine_age)
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
    let (root, key) = pool_key(remote, root, file);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_check_gets_more_time_than_an_interactive_query() {
        assert_eq!(budget_for("textDocument/hover").as_secs(), 60);
        assert_eq!(budget_for("textDocument/diagnostic").as_secs(), 300);
        assert_eq!(budget_for("prodCode/structuralReplace").as_secs(), 900);
    }
}
