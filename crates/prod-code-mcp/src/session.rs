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
use prod_code_protocol::{
    AnyStream, HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage,
    supported_protocol_versions, validate_selected_protocol_version,
};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tokio_util::codec::Framed;
use url::Url;

pub struct LspSession {
    remote: SocketAddr,
    framed: Framed<AnyStream, ProdCodeCodec>,
    root: PathBuf,
    /// The documents open in the server, by URI: the file, and a hash of the disk text last
    /// sent for it (`None` for a proposed text, which is not the file's).
    opened: HashMap<String, (PathBuf, Option<u64>)>,
    next_id: i64,
    /// The engine the gateway chose for this session.
    pub engine: String,
    /// When the gateway loaded that engine; `None` when it does not say (#381).
    engine_loaded: Option<std::time::Instant>,
    /// Whether the gateway holds this engine's index questions until its server is ready, so
    /// that an empty answer is final (#391).
    index_gated: bool,
}

/// How long after its engine was loaded an empty `workspace/symbol` answer may still be early:
/// a language server indexes after it starts (#381).
pub const INDEXING_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

// Includes connection, sync and cold engine loading. A dead gateway must not hold a caller
// indefinitely before its first LSP request even starts (#430).
const OPEN_BUDGET: std::time::Duration = std::time::Duration::from_secs(180);

fn timeout_error(stage: &str, budget: std::time::Duration) -> anyhow::Error {
    anyhow!(
        "timeout {stage} after {} ms; the gateway may be loading or short of capacity. \
             Run `prod-code cluster` to inspect it, then retry or select another node",
        budget.as_millis()
    )
}

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
        // The gateway may hold an index question while the server finishes indexing (#391).
        m if prod_code_protocol::readiness::needs_index(m) => 120,
        _ => 60,
    })
}

/// Notes, per checkout, of index questions the gateway answered while the language server was
/// still loading or indexing; a tool's answer carries them (#391).
static INDEXING_NOTES: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, Vec<String>>>> =
    std::sync::LazyLock::new(Default::default);

fn record_indexing_note(root: &Path, note: String) {
    let mut notes = INDEXING_NOTES.lock().unwrap_or_else(|e| e.into_inner());
    let notes = notes.entry(root.to_path_buf()).or_default();
    if !notes.contains(&note) {
        notes.push(note);
    }
}

/// The indexing notes recorded for the checkout at `root` since the last call.
pub fn take_indexing_notes(root: &Path) -> Vec<String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    INDEXING_NOTES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&root)
        .unwrap_or_default()
}

impl LspSession {
    /// Whether the gateway holds this session's index questions until its server is ready
    /// (#391).
    pub fn index_gated(&self) -> bool {
        self.index_gated
    }

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
        Self::open_with_budget(remote, root, hint, purpose, OPEN_BUDGET).await
    }

    async fn open_with_budget(
        remote: SocketAddr,
        root: &Path,
        hint: Option<&Path>,
        purpose: Option<&str>,
        budget: std::time::Duration,
    ) -> Result<Self> {
        tokio::time::timeout(budget, Self::open_inner(remote, root, hint, purpose))
            .await
            .map_err(|_| {
                timeout_error("opening the session (connect, sync or engine load)", budget)
            })?
    }

    async fn open_inner(
        mut remote: SocketAddr,
        root: &Path,
        hint: Option<&Path>,
        purpose: Option<&str>,
    ) -> Result<Self> {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let root_str = root.to_string_lossy().to_string();
        let identity: WorkspaceIdentity = workspace_identity(&root);
        let mut redirect_count = 0;
        let (framed, handshake) = loop {
            let stream = prod_code_protocol::transport::connect(remote)
                .await
                .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
            let mut framed = Framed::new(stream, ProdCodeCodec::new());
            push_workspace_sync(&mut framed, &root, &identity, None)
                .await
                .context("pre-flight workspace sync failed")?;
            let (engine_subpath, mut engine) = engine_project(&root, hint.unwrap_or(&root));
            if let Some(h) = hint
                && let Some(own) = crate::sync::engine_for_file(h)
                && engine == crate::sync::expected_engine(&root)
                && Some(own) != engine
            {
                engine = Some(own);
            }
            let preferred_engine = engine.map(str::to_string);
            let supported_versions = supported_protocol_versions();
            framed
                .send(WireMessage::HandshakeRequest(HandshakeRequest {
                    protocol_version: PROTOCOL_VERSION,
                    supported_versions: Some(supported_versions.clone()),
                    capabilities: Some(prod_code_protocol::ClientCapabilities {
                        direct_edit: true,
                        watch_files: true,
                        indexing_status: true,
                        shadow_runs: true,
                        multi_root: true,
                        sync_chunking: true,
                        unix_socket_local: cfg!(unix),
                    }),
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
                    redirect_count,
                }))
                .await?;
            let handshake = match framed.next().await {
                Some(Ok(WireMessage::HandshakeResponse(resp))) => resp,
                Some(Ok(WireMessage::Redirect { target_addr, reason })) => {
                    redirect_count += 1;
                    if redirect_count > 3 {
                        anyhow::bail!("too many gateway redirects: {reason:?}");
                    }
                    tracing::info!(%target_addr, ?reason, "received transparent redirect from gateway");
                    if let Ok(addr) = target_addr.parse::<SocketAddr>() {
                        remote = addr;
                        crate::cluster::remember_placement(&identity.name, remote);
                        continue;
                    } else {
                        anyhow::bail!("invalid redirect target address: {target_addr}");
                    }
                }
                Some(Ok(WireMessage::Disconnect { reason })) => {
                    anyhow::bail!("gateway refused the session: {reason}")
                }
                other => anyhow::bail!("unexpected handshake response: {other:?}"),
            };
            validate_selected_protocol_version(handshake.protocol_version, &supported_versions)
                .context("gateway returned an incompatible MCP handshake response")?;
            break (framed, handshake);
        };
        let folder_name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_string();
        let mut session = Self {
            remote,
            framed,
            root,
            opened: HashMap::new(),
            next_id: 1,
            engine: handshake.detected_engine,
            engine_loaded: handshake.engine_age_ms.and_then(|ms| {
                std::time::Instant::now().checked_sub(std::time::Duration::from_millis(ms))
            }),
            index_gated: handshake.index_gated,
        };
        // Files the gateway lost from its copy (#262) that the sync above did not carry: the
        // next sync sends them again.
        resend_lost_files(
            &session.root,
            &gateway_node(&session.framed),
            &handshake.stale_paths,
        );
        let root_uri = prod_code_protocol::path::file_uri(&session.root);
        let init = serde_json::json!({
            "processId": null,
            "rootUri": root_uri,
            "workspaceFolders": [{ "name": folder_name, "uri": root_uri }],
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

    pub fn remote(&self) -> SocketAddr {
        self.remote
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
                    // The server was still loading or indexing when the gateway asked it: the
                    // answer that follows may be incomplete, and the tool says so (#391).
                    if val.get("method").and_then(|m| m.as_str())
                        == Some(prod_code_protocol::readiness::BUSY_NOTIFICATION)
                        && let Some(busy) = val.get("params").and_then(|p| {
                            serde_json::from_value::<prod_code_protocol::readiness::Busy>(p.clone())
                                .ok()
                        })
                    {
                        record_indexing_note(&self.root, busy.describe());
                        continue;
                    }
                    // An answer has no method: a request from the server that carries the same
                    // id is not it (#391).
                    if val.get("method").is_some()
                        || val.get("id").and_then(|i| i.as_i64()) != Some(id)
                    {
                        continue;
                    }
                    if let Some(err) = val.get("error") {
                        let message = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error");
                        anyhow::bail!("{method} failed: {message}");
                    }
                    return val
                        .get("result")
                        .cloned()
                        .with_context(|| format!("{method} response has no result"));
                }
                Ok(Some(Ok(WireMessage::Ping))) => {
                    let _ = self.framed.send(WireMessage::Pong).await;
                }
                Ok(Some(Ok(WireMessage::Redirect { target_addr, reason }))) => {
                    tracing::info!(%target_addr, ?reason, "received dynamic redirect mid-session from gateway");
                    let ws_identity = workspace_identity(&self.root);
                    if let Ok(addr) = target_addr.parse::<SocketAddr>() {
                        crate::cluster::remember_placement(&ws_identity.name, addr);
                    }
                    anyhow::bail!("session rebalanced to {target_addr}: {}", reason.unwrap_or_default());
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
        crate::call_tree::clear_call_hierarchy_cache_for(&self.root);
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
type SessionSlot = Arc<tokio::sync::Mutex<Option<LspSession>>>;

fn pool() -> &'static tokio::sync::Mutex<HashMap<String, SessionSlot>> {
    static POOL: OnceLock<tokio::sync::Mutex<HashMap<String, SessionSlot>>> = OnceLock::new();
    POOL.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

async fn session_slot(key: &str) -> SessionSlot {
    let mut sessions = pool().lock().await;
    Arc::clone(sessions.entry(key.to_string()).or_default())
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
        || text.contains("rebalanced to")
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
    let (subpath, mut engine) = engine_project(&root, file);
    if let Some(own) = crate::sync::engine_for_file(file)
        && engine == crate::sync::expected_engine(&root)
        && Some(own) != engine
    {
        engine = Some(own);
    }
    let key = format!(
        "{remote}|{}|{}|{}",
        root.display(),
        subpath.unwrap_or_default(),
        engine.unwrap_or_default()
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
    let slot = pool().lock().await.get(&key).cloned()?;
    // Metadata is advisory; it must not queue behind a query just to report its age.
    let session = slot.try_lock().ok()?;
    session.as_ref().and_then(LspSession::engine_age)
}

/// [`LspSession::index_gated`] of the pooled session that answers about `file` in the checkout
/// at `root`; `false` when there is none yet (#391).
pub async fn pooled_index_gated(remote: SocketAddr, root: &Path, file: &Path) -> bool {
    let (_, key) = pool_key(remote, root, file);
    let Some(slot) = pool().lock().await.get(&key).cloned() else {
        return false;
    };
    slot.try_lock()
        .ok()
        .and_then(|session| session.as_ref().map(LspSession::index_gated))
        .unwrap_or(false)
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
    pooled_query_with_budget(
        remote,
        root,
        file,
        method,
        params,
        budget_for(method).max(OPEN_BUDGET),
    )
    .await
}

async fn pooled_query_with_budget(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    method: &str,
    params: serde_json::Value,
    budget: std::time::Duration,
) -> Result<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + budget;
    let (root, key) = pool_key(remote, root, file);
    let slot = session_slot(&key).await;
    let mut stored = tokio::time::timeout_at(deadline, slot.lock())
        .await
        .map_err(|_| timeout_error("waiting for another query in this workspace", budget))?;
    // The slot stays empty while the request owns the connection. Cancellation drops the
    // connection too, rather than caching an interrupted sync or an unread reply (#430).
    let mut session = stored.take();
    let result = tokio::time::timeout_at(deadline, async {
        for attempt in 0..2 {
            let mut target_remote = remote;
            let ws_identity = workspace_identity(&root);
            if let Some(remembered) = crate::cluster::remembered_node(&ws_identity.name) {
                target_remote = remembered;
            }
            if session.is_none() {
                session = Some(LspSession::open(target_remote, &root, Some(file)).await?);
                crate::watch::mark_synced(&root, crate::watch::current_generation(&root));
            }
            let current = session.as_mut().expect("just inserted");
            let generation = crate::watch::current_generation(&root);
            let result = async {
                if crate::watch::sync_due(&root, generation) {
                    current.refresh().await?;
                    crate::watch::mark_synced(&root, generation);
                    crate::call_tree::clear_call_hierarchy_cache_for(&root);
                }
                current.query(file, method, params.clone()).await
            }
            .await;
            match result {
                Ok(value) => return Ok(value),
                Err(err) if is_connection_error(&err) => {
                    session = None;
                    // A timeout is already a spent budget; repeating the same analysis
                    // immediately used to double the wait. The next call may retry it.
                    let timeout = format!("{err:#}").to_ascii_lowercase().contains("timeout");
                    if attempt == 0 && !timeout && method != "workspace/executeCommand" {
                        continue;
                    }
                    return Err(err);
                }
                Err(err) => return Err(err),
            }
        }
        unreachable!("two attempts always return")
    })
    .await;
    match result {
        Ok(result) => {
            *stored = session;
            result
        }
        Err(_) => Err(timeout_error(
            &format!("running {method} (including sync and engine load)"),
            budget,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Connects a session directly to a framed peer that replies once, without the testkit's
    /// normal result wrapper. This keeps malformed JSON-RPC envelopes observable at the
    /// session boundary.
    async fn raw_session(reply: serde_json::Value) -> LspSession {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut peer = Framed::new(socket, ProdCodeCodec::new());
            let request = match peer.next().await {
                Some(Ok(WireMessage::LspPayload(json))) => {
                    serde_json::from_str::<serde_json::Value>(&json).unwrap()
                }
                other => panic!("expected an LSP request, got {other:?}"),
            };
            let mut reply = reply;
            reply["jsonrpc"] = serde_json::json!("2.0");
            reply["id"] = request["id"].clone();
            peer.send(WireMessage::LspPayload(reply.to_string()))
                .await
                .unwrap();
        });
        LspSession {
            remote: addr,
            framed: Framed::new(
                AnyStream::connect(addr).await.unwrap(),
                ProdCodeCodec::new(),
            ),
            root: PathBuf::new(),
            opened: HashMap::new(),
            next_id: 1,
            engine: "test".to_string(),
            engine_loaded: None,
            index_gated: false,
        }
    }

    #[tokio::test]
    async fn a_missing_result_is_an_actionable_method_error() {
        let mut session = raw_session(serde_json::json!({})).await;
        let error = session
            .request("textDocument/implementation", serde_json::json!({}))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(error, "textDocument/implementation response has no result");
    }

    #[tokio::test]
    async fn explicit_null_and_ordinary_results_are_preserved() {
        for expected in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!({ "found": true }),
            serde_json::json!(7),
        ] {
            let mut session = raw_session(serde_json::json!({ "result": expected.clone() })).await;
            assert_eq!(
                session
                    .request("textDocument/implementation", serde_json::json!({}))
                    .await
                    .unwrap(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn an_error_envelope_remains_an_error() {
        let mut session = raw_session(serde_json::json!({
            "error": { "code": -32001, "message": "server refused" }
        }))
        .await;
        let error = session
            .request("textDocument/implementation", serde_json::json!({}))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(error, "textDocument/implementation failed: server refused");
    }

    #[test]
    fn a_full_check_gets_more_time_than_an_interactive_query() {
        assert_eq!(budget_for("textDocument/hover").as_secs(), 60);
        assert_eq!(budget_for("textDocument/diagnostic").as_secs(), 300);
        assert_eq!(budget_for("prodCode/structuralReplace").as_secs(), 900);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn opening_a_silent_gateway_has_a_deadline() {
        let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hold = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let _socket = socket;
            std::future::pending::<()>().await;
        });
        let result = LspSession::open_with_budget(
            addr,
            &ws.root(),
            None,
            None,
            std::time::Duration::from_millis(100),
        )
        .await;
        let error = result.err().expect("silent peer must time out").to_string();
        assert!(
            error.contains("opening the session") && error.contains("prod-code cluster"),
            "{error}"
        );
        hold.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_incompatible_gateway_selection_is_refused_before_initialize() {
        let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut framed = Framed::new(socket, ProdCodeCodec::new());
            loop {
                match framed.next().await.unwrap().unwrap() {
                    WireMessage::SyncProbeRequest(req) => {
                        framed
                            .send(WireMessage::SyncProbeResponse(
                                prod_code_protocol::SyncProbeResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    seeded: false,
                                    files_deleted: 0,
                                    missing: Vec::new(),
                                },
                            ))
                            .await
                            .unwrap();
                    }
                    WireMessage::HandshakeRequest(req) => {
                        assert_eq!(req.supported_versions, Some(supported_protocol_versions()));
                        framed
                            .send(WireMessage::HandshakeResponse(
                                prod_code_protocol::HandshakeResponse {
                                    protocol_version: 2,
                                    server_pid: 1,
                                    session_id: 1,
                                    server_workspace_root: req.client_workspace_root,
                                    detected_engine: "rust".to_string(),
                                    stale_paths: Vec::new(),
                                    engine_age_ms: None,
                                    index_gated: false,
                                    capabilities: None,
                                },
                            ))
                            .await
                            .unwrap();
                        break;
                    }
                    other => panic!("unexpected setup message: {other:?}"),
                }
            }
            assert!(
                framed.next().await.is_none(),
                "an incompatible selection must close before initialize"
            );
        });

        let result = LspSession::open_with_budget(
            addr,
            &ws.root(),
            None,
            None,
            std::time::Duration::from_secs(3),
        )
        .await;
        let error = result
            .err()
            .expect("incompatible selection must fail")
            .to_string();
        assert!(
            error.contains("incompatible MCP handshake response"),
            "{error}"
        );
        peer.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_queued_query_times_out_without_discarding_the_owner() {
        let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
        let gateway = prod_code_testkit::ScriptedGateway::start(|_, _| serde_json::json!([])).await;
        let root = ws.root();
        let file = ws.path("src/lib.rs");
        pooled_query(
            gateway.addr(),
            &root,
            &file,
            "textDocument/documentSymbol",
            serde_json::json!({}),
        )
        .await
        .unwrap();
        let (_, key) = pool_key(gateway.addr(), &root, &file);
        let slot = session_slot(&key).await;
        let held = slot.lock().await;
        assert!(held.is_some());
        let error = pooled_query_with_budget(
            gateway.addr(),
            &root,
            &file,
            "textDocument/documentSymbol",
            serde_json::json!({}),
            std::time::Duration::from_millis(40),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("waiting for another query"), "{error}");
        assert!(
            held.is_some(),
            "the queued caller must not invalidate its owner"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_timed_out_query_discards_its_connection_without_replaying_it() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
        let slow = Arc::new(AtomicBool::new(false));
        let handshakes = Arc::new(AtomicUsize::new(0));
        let (delay, counted) = (Arc::clone(&slow), Arc::clone(&handshakes));
        let gateway = prod_code_testkit::ScriptedGateway::start(move |method, _| {
            if method == "prod-code/handshake" {
                counted.fetch_add(1, Ordering::SeqCst);
            }
            if method == "textDocument/documentSymbol" && delay.swap(false, Ordering::SeqCst) {
                tokio::task::block_in_place(|| {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                });
            }
            serde_json::json!([])
        })
        .await;
        let root = ws.root();
        let file = ws.path("src/lib.rs");
        pooled_query(
            gateway.addr(),
            &root,
            &file,
            "textDocument/documentSymbol",
            serde_json::json!({}),
        )
        .await
        .unwrap();
        slow.store(true, Ordering::SeqCst);
        let before = gateway.calls();
        let error = pooled_query_with_budget(
            gateway.addr(),
            &root,
            &file,
            "textDocument/documentSymbol",
            serde_json::json!({}),
            std::time::Duration::from_millis(100),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("timeout running textDocument/documentSymbol"),
            "{error}"
        );
        assert_eq!(
            gateway.calls(),
            before + 1,
            "a timeout must not replay the request"
        );
        let (_, key) = pool_key(gateway.addr(), &root, &file);
        assert!(session_slot(&key).await.lock().await.is_none());
        pooled_query(
            gateway.addr(),
            &root,
            &file,
            "textDocument/documentSymbol",
            serde_json::json!({}),
        )
        .await
        .unwrap();
        assert_eq!(
            handshakes.load(Ordering::SeqCst),
            2,
            "the next query reconnects"
        );
    }

    #[test]
    fn rebalance_error_is_classified_as_connection_error() {
        let err = anyhow::anyhow!("session rebalanced to 127.0.0.1:9400: congested gateway");
        assert!(is_connection_error(&err));
    }
}
