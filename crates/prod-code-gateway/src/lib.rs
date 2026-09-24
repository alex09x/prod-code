//! prod-code gateway daemon: multi-tenant server for remote code intelligence over 10 GbE LAN.
//!
//! The daemon is a library with a thin binary on top, so that the pieces a socket normally
//! stands in front of — the workspace manager, the dispatch, the language server backends —
//! can be exercised directly by tests.

pub mod backend;
pub mod detect;
pub mod embed;
pub mod memory;
mod metrics;
pub mod priming;
pub mod search;
pub mod shadow;
pub mod workspace;

pub use detect::detect_engine;

use anyhow::Result;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ClusterResponse, ExecChanges, ExecChunk, ExecExit, ExecRequest, FileDelta, FileStamp,
    HandshakeResponse, LoadedWorkspaceInfo, NodeGossip, PROTOCOL_VERSION, PathTranslator, PeerInfo,
    PlaceRequest, PlaceResponse, ProdCodeCodec, StatusResponse, SyncProbeRequest,
    SyncProbeResponse, SyncRequest, SyncResponse, WireMessage, content_hash,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;
use workspace::{SessionView, WorkspaceManager};

pub static NEXT_REQ_ID: AtomicU64 = AtomicU64::new(1);
pub static ACTIVE_QUERIES: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_QUERIES: AtomicU64 = AtomicU64::new(0);
pub static SLOW_QUERIES: AtomicU64 = AtomicU64::new(0);

#[derive(Parser, Debug)]
#[command(
    name = "prod-code-server",
    author,
    version,
    about = "Remote Code Intelligence Gateway"
)]
pub struct ServerCli {
    /// Bind address (IP:port). Defaults to 0.0.0.0:9400.
    #[arg(short, long, env = "PROD_CODE_BIND", default_value = "0.0.0.0:9400")]
    pub bind: SocketAddr,

    /// Workspace root storage directory on server.
    #[arg(
        short,
        long,
        env = "PROD_CODE_STORAGE",
        default_value = "/srv/prod-code/workspaces"
    )]
    pub storage: PathBuf,

    /// Unload a workspace's engine after this many seconds without a session (0 disables).
    #[arg(long, env = "PROD_CODE_IDLE_EVICT_SECS", default_value_t = 1800)]
    pub idle_evict_secs: u64,

    /// Delete `<repo>--wt-*` workspace directories unused for this many days (0 disables).
    #[arg(long, env = "PROD_CODE_PRUNE_WORKTREE_DAYS", default_value_t = 7)]
    pub prune_worktree_days: u64,

    /// Only serve these engines (comma-separated: rust, go, cpp, swift, python, typescript).
    /// The node advertises nothing else, so placement never sends other work here, and a
    /// handshake for another engine is refused. Empty: every installed engine.
    #[arg(long, env = "PROD_CODE_ENGINES", value_delimiter = ',')]
    pub engines: Vec<String>,

    /// Directory for the overlays of shadow runs (one upper directory per hypothesis, holding
    /// what its build wrote). Default: `shadow` next to the storage directory; a tmpfs path
    /// (`/dev/shm/prod-code-shadow`) keeps hypothesis builds in RAM.
    #[arg(long, env = "PROD_CODE_SHADOW_DIR")]
    pub shadow_dir: Option<PathBuf>,

    /// Other gateways of the cluster (`host:port,host:port`); membership then spreads by
    /// gossip, so listing one live peer is enough.
    #[arg(long, env = "PROD_CODE_PEERS", default_value = "")]
    pub peers: String,

    /// The address peers and clients reach this gateway at (`host:port`); detected from the
    /// primary interface when absent.
    #[arg(long, env = "PROD_CODE_ADVERTISE")]
    pub advertise: Option<String>,
}

pub struct ServerState {
    pub start_time: Instant,
    pub server_pid: u32,
    pub next_session_id: AtomicU64,
    pub active_sessions: AtomicUsize,
    pub storage_root: PathBuf,
    pub workspace_manager: Arc<WorkspaceManager>,
    /// This gateway's address as peers and clients reach it.
    pub advertise: tokio::sync::RwLock<String>,
    /// Peers to gossip with: configured ones plus those learned from gossip.
    pub peers: tokio::sync::RwLock<std::collections::BTreeSet<String>>,
    /// Latest heartbeat from every peer and when it arrived.
    pub cluster: tokio::sync::RwLock<std::collections::HashMap<String, PeerEntry>>,
    /// Usage metrics (JSONL on disk + in-memory ring).
    pub metrics: Arc<metrics::Metrics>,
    /// Engines this node serves (`--engines`); empty means every installed engine.
    pub engine_allowlist: Vec<String>,
    /// Where shadow runs keep the upper directories of their overlays (`--shadow-dir`).
    pub shadow_root: PathBuf,
    /// Per-workspace declaration indexes for `code_search`.
    pub search_indexes: search::SearchIndexes,
}

/// Who a session belongs to, for metrics.
pub struct SessionMeta {
    pub session_id: u64,
    pub client_name: String,
    pub agent: String,
    pub host: String,
    pub client_addr: String,
    pub workspace: String,
    pub engine: String,
    pub engine_root: PathBuf,
    pub metrics: Arc<metrics::Metrics>,
}

/// An LSP request awaiting its answer.
pub struct PendingRequest {
    pub method: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub start: Instant,
}

/// A peer's latest heartbeat.
pub struct PeerEntry {
    pub gossip: NodeGossip,
    pub last_seen: Instant,
}

/// How long a silent peer still counts as alive.
const PEER_ALIVE: std::time::Duration = std::time::Duration::from_secs(30);
/// Heartbeat period.
const GOSSIP_PERIOD: std::time::Duration = std::time::Duration::from_secs(5);

impl ServerState {
    pub fn new(storage_root: PathBuf) -> Self {
        let metrics_dir = storage_root
            .parent()
            .map(|p| p.join("metrics"))
            .unwrap_or_else(|| storage_root.join("metrics"));
        Self {
            start_time: Instant::now(),
            server_pid: std::process::id(),
            next_session_id: AtomicU64::new(1),
            active_sessions: AtomicUsize::new(0),
            shadow_root: shadow::default_root(&storage_root),
            search_indexes: search::SearchIndexes::with_model_dir(embed::model_dir(&storage_root)),
            storage_root,
            workspace_manager: Arc::new(WorkspaceManager::new()),
            advertise: tokio::sync::RwLock::new(String::new()),
            peers: tokio::sync::RwLock::new(std::collections::BTreeSet::new()),
            cluster: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            metrics: Arc::new(metrics::Metrics::new(metrics_dir)),
            engine_allowlist: Vec::new(),
        }
    }

    /// Whether this node serves `engine` (an [`EngineKind`] name such as `rust`).
    pub fn serves_engine(&self, engine: &str) -> bool {
        self.engine_allowlist.is_empty()
            || self
                .engine_allowlist
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(engine))
    }

    /// The installed engines this node advertises, narrowed by `--engines`.
    fn advertised_engines(&self) -> Vec<String> {
        cached_available_engines()
            .into_iter()
            .filter(|entry| {
                let name = entry.split(' ').next().unwrap_or(entry.as_str());
                let name = name.strip_suffix("-lsp").unwrap_or(name);
                self.serves_engine(name)
            })
            .collect()
    }

    /// This node's heartbeat.
    pub async fn own_gossip(&self) -> NodeGossip {
        let status = self.status().await;
        let workspaces = self
            .workspace_manager
            .loaded_summary()
            .await
            .into_iter()
            .map(|(name, engine, sessions)| LoadedWorkspaceInfo {
                name,
                engine,
                sessions,
            })
            .collect();
        let peers = self.peers.read().await.iter().cloned().collect();
        NodeGossip {
            addr: self.advertise.read().await.clone(),
            status,
            workspaces,
            peers,
            sent_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        }
    }

    /// Records a peer's heartbeat and learns the peers it knows.
    pub async fn absorb_gossip(&self, gossip: NodeGossip) {
        let me = self.advertise.read().await.clone();
        if gossip.addr.is_empty() || gossip.addr == me {
            return;
        }
        {
            let mut peers = self.peers.write().await;
            peers.insert(gossip.addr.clone());
            for p in &gossip.peers {
                if *p != me && peers.len() < 64 {
                    peers.insert(p.clone());
                }
            }
        }
        self.cluster.write().await.insert(
            gossip.addr.clone(),
            PeerEntry {
                gossip,
                last_seen: Instant::now(),
            },
        );
    }

    /// The cluster as this node sees it, itself first.
    pub async fn cluster_view(&self) -> ClusterResponse {
        let own = self.own_gossip().await;
        let mut nodes = vec![PeerInfo {
            addr: own.addr.clone(),
            status: own.status,
            workspaces: own.workspaces,
            last_seen_secs: 0,
            alive: true,
        }];
        let cluster = self.cluster.read().await;
        for entry in cluster.values() {
            let age = entry.last_seen.elapsed();
            nodes.push(PeerInfo {
                addr: entry.gossip.addr.clone(),
                status: entry.gossip.status.clone(),
                workspaces: entry.gossip.workspaces.clone(),
                last_seen_secs: age.as_secs(),
                alive: age < PEER_ALIVE,
            });
        }
        nodes[1..].sort_by(|a, b| a.addr.cmp(&b.addr));
        ClusterResponse {
            this_node: own.addr,
            nodes,
        }
    }

    /// Where `workspace_name` should live: the node that already holds it (this one first),
    /// otherwise the quietest live node that serves `engine`; a node loaded but idle on an
    /// overloaded gateway moves to a much quieter one.
    pub async fn place(&self, req: &PlaceRequest) -> PlaceResponse {
        let view = self.cluster_view().await;
        let engine = req.engine.as_deref();
        // A node started with `--engines swift` advertises one engine and serves nothing
        // else. A workspace whose engine the client could not determine must not be sent
        // there: it would be refused at the handshake, or worse, accepted by an older
        // gateway that does not know it is specialised.
        let capable = |n: &PeerInfo| match engine {
            Some(e) => cluster_supports_engine(&n.status, e),
            None => n.status.detected_engines.len() > 1,
        };
        let load = |n: &PeerInfo| n.status.load_per_cpu().unwrap_or(f64::MAX);
        let quietest = view
            .nodes
            .iter()
            .filter(|n| n.alive && capable(n))
            .min_by(|a, b| {
                load(a)
                    .partial_cmp(&load(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        let holder = view.nodes.iter().find(|n| {
            n.alive && capable(n) && n.workspaces.iter().any(|w| w.name == req.workspace_name)
        });
        if let Some(h) = holder {
            let idle = h
                .workspaces
                .iter()
                .find(|w| w.name == req.workspace_name)
                .map(|w| w.sessions == 0)
                .unwrap_or(true);
            if let Some(q) = quietest
                && idle
                && q.addr != h.addr
                && load(h) > 1.0
                && load(q) < load(h) * 0.5
            {
                return PlaceResponse {
                    node: Some(q.addr.clone()),
                    reason: format!(
                        "moved from {} (load {:.2}/cpu, idle) to the quieter {} ({:.2}/cpu)",
                        h.addr,
                        load(h),
                        q.addr,
                        load(q)
                    ),
                };
            }
            return PlaceResponse {
                node: Some(h.addr.clone()),
                reason: format!("already loaded on {}", h.addr),
            };
        }
        match quietest {
            Some(q) => PlaceResponse {
                node: Some(q.addr.clone()),
                reason: format!(
                    "quietest node serving {} ({:.2}/cpu)",
                    engine.unwrap_or("any engine"),
                    load(q)
                ),
            },
            None => PlaceResponse {
                node: None,
                reason: format!("no live node serves {}", engine.unwrap_or("this workspace")),
            },
        }
    }

    pub async fn status(&self) -> StatusResponse {
        StatusResponse {
            server_pid: self.server_pid,
            uptime_seconds: self.start_time.elapsed().as_secs(),
            active_sessions: self.active_sessions.load(Ordering::Relaxed),
            loaded_workspaces: self.workspace_manager.loaded_count().await,
            detected_engines: self.advertised_engines(),
            memory_rss_bytes: memory::get_process_rss_bytes(),
            total_queries: TOTAL_QUERIES.load(Ordering::Relaxed),
            active_queries: ACTIVE_QUERIES.load(Ordering::Relaxed),
            load_average_millis: memory::load_average_1m().map(|l| (l * 1000.0) as u32),
            cpu_count: std::thread::available_parallelism().ok().map(|n| n.get()),
        }
    }
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<usize> {
    let mut copied = 0;
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git"
            || name_str == "target"
            || name_str == "node_modules"
            || name_str == workspace::LAST_USED_MARKER
        {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            copied += copy_tree(&from, &to)?;
        } else if from.is_file() {
            std::fs::copy(&from, &to)?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// Build products, dependency trees and virtual environments: per-node caches, not sources. They
/// never travel back to the client, and they are not the client's to delete.
fn is_node_cache(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "target"
            | "node_modules"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
            | ".tox"
            | ".nox"
            | "build"
            | ".build"
            | "dist"
            | ".cache"
            | ".next"
            | ".turbo"
            | "coverage"
            | "DerivedData"
            | ".swiftpm"
            | ".gradle"
    ) || name == workspace::LAST_USED_MARKER
}

fn walk_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if is_node_cache(&name) {
            continue;
        }
        if path.is_dir() {
            walk_files(root, &path, out);
        } else if path.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, path));
        }
    }
}

/// Removes `dir` and every parent left empty by that, up to but not including `root`: a directory
/// whose last file was deleted or moved away locally goes away on the copy too (#124). A
/// directory that still holds anything stops the climb.
fn prune_empty_parents(root: &std::path::Path, dir: Option<&std::path::Path>) {
    let mut dir = dir;
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            return;
        }
        dir = d.parent();
    }
}

/// Removes every directory under `dir` that holds no file, however deeply, except the per-node
/// caches: what an earlier deletion left behind before empty directories were pruned (#124).
/// Returns whether `dir` itself is now empty.
fn prune_empty_dirs(root: &std::path::Path, dir: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut empty = true;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() && !path.is_symlink() && !is_node_cache(&name) {
            if prune_empty_dirs(root, &path) {
                let _ = std::fs::remove_dir(&path);
            } else {
                empty = false;
            }
        } else {
            empty = false;
        }
    }
    empty && dir != root
}

/// Compares the workspace directory with the client's manifest: deletes files the client does
/// not have, and the directories that leaves empty, and returns `(missing, deleted)` where
/// `missing` are manifest paths the server lacks or holds with different content.
fn reconcile_manifest(root: &std::path::Path, stamps: &[FileStamp]) -> (Vec<String>, Vec<String>) {
    let wanted: std::collections::HashMap<&str, &FileStamp> = stamps
        .iter()
        .map(|s| (s.relative_path.as_str(), s))
        .collect();
    let mut present = Vec::new();
    walk_files(root, root, &mut present);
    let mut missing = Vec::new();
    let mut deleted = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (rel, path) in &present {
        match wanted.get(rel.as_str()) {
            None => {
                if std::fs::remove_file(path).is_ok() {
                    deleted.push(rel.clone());
                }
            }
            Some(stamp) => {
                seen.insert(stamp.relative_path.as_str());
                let same = std::fs::metadata(path)
                    .map(|m| m.len() == stamp.size)
                    .unwrap_or(false)
                    && std::fs::read(path)
                        .map(|bytes| content_hash(&bytes) == stamp.hash)
                        .unwrap_or(false);
                if !same {
                    missing.push(rel.clone());
                }
            }
        }
    }
    for stamp in stamps {
        if !seen.contains(stamp.relative_path.as_str()) {
            missing.push(stamp.relative_path.clone());
        }
    }
    prune_empty_dirs(root, root);
    missing.sort();
    missing.dedup();
    (missing, deleted)
}

/// Answers a manifest probe: seeds a fresh workspace from the origin repository's copy,
/// reconciles it with the client's manifest, and reports what the client must still upload.
pub async fn apply_sync_probe(
    storage_root: &std::path::Path,
    workspace_manager: &WorkspaceManager,
    req: SyncProbeRequest,
) -> SyncProbeResponse {
    let start = Instant::now();
    let target = workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let fresh = !target.exists()
        || std::fs::read_dir(&target)
            .map(|mut d| d.next().is_none())
            .unwrap_or(true);
    let mut seeded = false;
    if fresh && let Some(seed) = req.seed_from.as_deref() {
        let seed_dir = storage_root.join(workspace::sanitize_identifier(seed.trim()));
        if seed_dir.is_dir() && seed_dir != target {
            let (from, to) = (seed_dir.clone(), target.clone());
            match tokio::task::spawn_blocking(move || copy_tree(&from, &to)).await {
                Ok(Ok(files)) => {
                    seeded = true;
                    tracing::info!(
                        workspace = %target.display(),
                        seed = %seed_dir.display(),
                        files,
                        "🌱 [SEED] new worktree workspace seeded from origin copy"
                    );
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, seed = %seed_dir.display(), "seeding failed")
                }
                Err(e) => tracing::warn!(error = %e, "seeding task failed"),
            }
        }
    }
    let _ = std::fs::create_dir_all(&target);
    workspace::touch_last_used(&target);

    let root = target.clone();
    let stamps = req.files;
    let manifest_len = stamps.len();
    let (missing, deleted) =
        tokio::task::spawn_blocking(move || reconcile_manifest(&root, &stamps))
            .await
            .unwrap_or_default();

    if !deleted.is_empty()
        && let Some(ws) = workspace_manager.get_loaded(&target).await
    {
        for engine_lock in ws.mirrored_rust_engines() {
            let mut engine = engine_lock.lock().await;
            for rel in &deleted {
                let _ = engine.update_base(&target.join(rel), None);
            }
        }
    }

    tracing::info!(
        workspace = %target.display(),
        manifest = manifest_len,
        missing = missing.len(),
        deleted = deleted.len(),
        seeded,
        duration_ms = %format!("{}ms", start.elapsed().as_millis()),
        "🔎 [PROBE] manifest reconciled"
    );

    SyncProbeResponse {
        server_workspace_root: target.to_string_lossy().to_string(),
        seeded,
        files_deleted: deleted.len(),
        missing,
    }
}

/// Builds an LSP `WorkspaceEdit` (as `documentChanges`) from a refactoring outcome: every
/// rewritten file becomes one whole-file text edit, file moves become rename operations and
/// new files become create operations followed by their content.
/// Whether `path` is a source file the gateway may hand to clients: its own workspace
/// copies, toolchain and dependency caches under the home directory, and system SDK
/// locations. Nothing else on the host is readable this way.
fn is_readable_source_path(storage_root: &std::path::Path, path: &std::path::Path) -> bool {
    if !path.is_absolute() || !path.is_file() {
        return false;
    }
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if canonical.starts_with(storage_root) {
        return true;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        let allowed_home = [
            ".cargo/registry",
            ".cargo/git",
            ".rustup/toolchains",
            "go/pkg/mod",
            ".local/go",
            ".local/lib",
            ".npm-global/lib",
            ".bun/install",
            ".local/share/uv",
            "Library/Developer",
            "prod-code-storage",
        ];
        if allowed_home
            .iter()
            .any(|rel| canonical.starts_with(home.join(rel)))
        {
            return true;
        }
    }
    const SYSTEM_ROOTS: [&str; 10] = [
        "/snap",
        "/usr/include",
        "/usr/local/include",
        "/usr/lib",
        "/usr/local/lib",
        "/usr/local/go",
        "/usr/share",
        "/opt/homebrew",
        "/Applications/Xcode.app",
        "/Library/Developer",
    ];
    SYSTEM_ROOTS.iter().any(|root| canonical.starts_with(root))
}

/// Serves a `ReadFileRequest` under the readable-path policy, capped in size.
fn read_server_file(
    storage_root: &std::path::Path,
    req: &prod_code_protocol::ReadFileRequest,
) -> prod_code_protocol::ReadFileResponse {
    const DEFAULT_MAX: u64 = 2 * 1024 * 1024;
    let path = PathBuf::from(&req.path);
    let max = if req.max_bytes == 0 {
        DEFAULT_MAX
    } else {
        req.max_bytes.min(DEFAULT_MAX)
    };
    let mut resp = prod_code_protocol::ReadFileResponse {
        path: req.path.clone(),
        content: None,
        truncated: false,
        error: None,
    };
    if !is_readable_source_path(storage_root, &path) {
        resp.error = Some(format!(
            "{} is not a readable source location on this gateway",
            req.path
        ));
        return resp;
    }
    match std::fs::read(&path) {
        Ok(mut bytes) => {
            if bytes.len() as u64 > max {
                bytes.truncate(max as usize);
                resp.truncated = true;
            }
            resp.content = Some(bytes);
        }
        Err(e) => resp.error = Some(format!("cannot read {}: {e}", req.path)),
    }
    resp
}

/// A managed (out-of-process) language server the gateway can talk LSP to.
enum ManagedLsp<'a> {
    Go(&'a prod_code_engine_go::GoEngine),
    Generic(&'a prod_code_engine_generic::GenericLspEngine),
}

impl ManagedLsp<'_> {
    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let resp = match self {
            ManagedLsp::Go(engine) => engine.send_request(method, params).await?,
            ManagedLsp::Generic(engine) => engine.send_request(method, params).await?,
        };
        if let Some(err) = resp.get("error") {
            let message = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("request failed");
            anyhow::bail!("{method}: {message}");
        }
        Ok(resp
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    /// The diagnostics published for `uri`, waiting briefly for the first publication after a
    /// didOpen so quick fixes can be offered in a one-shot session.
    async fn diagnostics_for(&self, uri: &str) -> Vec<serde_json::Value> {
        match self {
            ManagedLsp::Go(_) => Vec::new(),
            ManagedLsp::Generic(engine) => {
                if engine.has_pull_diagnostics().await {
                    // Pull model (the native TypeScript server): ask for the document's
                    // diagnostics instead of waiting for a publication.
                    let pulled = engine
                        .send_request(
                            "textDocument/diagnostic",
                            serde_json::json!({ "textDocument": { "uri": uri } }),
                        )
                        .await
                        .ok()
                        .and_then(|r| r.get("result").cloned());
                    if let Some(items) = pulled
                        .as_ref()
                        .and_then(|r| r.get("items"))
                        .and_then(|i| i.as_array())
                    {
                        return items.clone();
                    }
                }
                for _ in 0..30 {
                    if engine.diagnostics_published(uri).await {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                engine.diagnostics_for(uri).await
            }
        }
    }
}

fn range_line(value: &serde_json::Value, end: bool) -> u64 {
    value
        .get(if end { "end" } else { "start" })
        .and_then(|p| p.get("line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0)
}

/// Stable id of a code action in a list: its index plus a slug of the title.
fn code_action_id(index: usize, action: &serde_json::Value) -> String {
    let title = action
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("action");
    let slug: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|p| !p.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("_");
    format!("{index}:{slug}")
}

/// Lists (`prodCode/assists`) or applies (`prodCode/applyAssist`) LSP code actions on a
/// managed language server. The listing is `[{id, kind, label}]` like the Rust engine's; the
/// apply step re-queries the actions, picks the one with the requested id, resolves it when
/// its edit is lazy and returns the WorkspaceEdit for the client to apply.
async fn lsp_code_actions(
    engine: &ManagedLsp<'_>,
    method: &str,
    params: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let uri = params
        .get("textDocument")
        .and_then(|t| t.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let range = params.get("range").cloned().unwrap_or(serde_json::json!({
        "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 }
    }));
    let (from, to) = (range_line(&range, false), range_line(&range, true));
    // Quick fixes are offered for the diagnostics in the requested range.
    let diagnostics: Vec<serde_json::Value> = engine
        .diagnostics_for(&uri)
        .await
        .into_iter()
        .filter(|d| {
            d.get("range")
                .map(|r| range_line(r, false) <= to && range_line(r, true) >= from)
                .unwrap_or(false)
        })
        .collect();
    let actions = engine
        .request(
            "textDocument/codeAction",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "range": range,
                "context": { "diagnostics": diagnostics },
            }),
        )
        .await?;
    let actions = actions.as_array().cloned().unwrap_or_default();
    match method {
        "prodCode/assists" => Ok(serde_json::Value::Array(
            actions
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let kind = a.get("kind").and_then(|k| k.as_str()).unwrap_or(
                        if a.get("command").is_some() && a.get("edit").is_none() {
                            "command"
                        } else {
                            "action"
                        },
                    );
                    serde_json::json!({
                        "id": code_action_id(i, a),
                        "kind": kind,
                        "label": a.get("title").and_then(|t| t.as_str()).unwrap_or(""),
                    })
                })
                .collect(),
        )),
        _ => {
            let wanted = params
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let found = actions
                .iter()
                .enumerate()
                .find(|(i, a)| code_action_id(*i, a) == wanted)
                .or_else(|| {
                    // Fall back to the title, so an id from an older listing still matches.
                    let title = wanted.split_once(':').map(|(_, t)| t).unwrap_or(&wanted);
                    actions
                        .iter()
                        .enumerate()
                        .find(|(i, a)| code_action_id(*i, a).ends_with(&format!(":{title}")))
                });
            let Some((_, action)) = found else {
                anyhow::bail!(
                    "no code action `{wanted}` at this position (available: {})",
                    actions
                        .iter()
                        .enumerate()
                        .map(|(i, a)| code_action_id(i, a))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            };
            if let Some(edit) = action.get("edit").filter(|e| !e.is_null()) {
                return Ok(edit.clone());
            }
            if action.get("title").is_some()
                && action.get("command").and_then(|c| c.as_str()).is_none()
            {
                let resolved = engine.request("codeAction/resolve", action.clone()).await?;
                if let Some(edit) = resolved.get("edit").filter(|e| !e.is_null()) {
                    return Ok(edit.clone());
                }
            }
            // A command-only action (clangd refactorings, some tsserver fixes): run it and
            // capture the edit the server pushes back.
            let command = match action.get("command") {
                Some(c) if c.is_object() => c.clone(),
                Some(c) if c.is_string() => serde_json::json!({
                    "command": c,
                    "arguments": action.get("arguments").cloned().unwrap_or(serde_json::json!([])),
                }),
                _ => serde_json::Value::Null,
            };
            let title = action
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or(&wanted)
                .to_string();
            if command.is_null() {
                anyhow::bail!("code action `{title}` carries neither an edit nor a command");
            }
            if let ManagedLsp::Generic(generic) = engine
                && let Some(edit) = generic
                    .execute_command_capturing_edit(command.clone())
                    .await?
            {
                return Ok(edit);
            }
            anyhow::bail!(
                "code action `{title}` ran the server command `{}` without producing an edit",
                command
                    .get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or("?")
            )
        }
    }
}

/// Whether a synced file configures a language server's view of the project, so that a
/// running engine must be restarted to honour it.
fn is_project_config_file(rel_path: &str) -> bool {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    matches!(
        name,
        "tsconfig.json"
            | "jsconfig.json"
            | "package.json"
            | "deno.json"
            | "pyproject.toml"
            | "setup.cfg"
            | "setup.py"
            | "pyrightconfig.json"
            | "uv.lock"
            | "CMakeLists.txt"
            | "compile_commands.json"
            | ".clangd"
            | "meson.build"
            | "Package.swift"
            | "Package.resolved"
            | "project.pbxproj"
            | "go.mod"
            | "go.work"
            | "Cargo.toml"
            | "rust-toolchain.toml"
            | "prod-code.toml"
    ) || name.starts_with("requirements")
}

/// LSP `languageId` for a server-side path, for the documents the gateway opens itself.
fn language_id_for_server_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rust",
        "go" => "go",
        "py" | "pyi" => "python",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        "swift" => "swift",
        _ => "plaintext",
    }
}

/// Runs `textDocument/rename` on a managed language server after opening every file that
/// references the symbol (pyright, for one, only rewrites open documents). Files the gateway
/// opened are closed again afterwards. Returns the server's response and how many files were
/// opened.
async fn rename_with_references_open(
    engine: &prod_code_engine_generic::GenericLspEngine,
    params: serde_json::Value,
) -> (anyhow::Result<serde_json::Value>, usize) {
    const MAX_OPENED: usize = 200;
    const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
    let own_uri = params
        .get("textDocument")
        .and_then(|t| t.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let refs_params = serde_json::json!({
        "textDocument": params.get("textDocument").cloned().unwrap_or_default(),
        "position": params.get("position").cloned().unwrap_or_default(),
        "context": { "includeDeclaration": true },
    });
    let mut opened: Vec<String> = Vec::new();
    if let Ok(refs) = engine
        .send_request("textDocument/references", refs_params)
        .await
    {
        let uris: std::collections::BTreeSet<String> = refs
            .get("result")
            .and_then(|r| r.as_array())
            .map(|locations| {
                locations
                    .iter()
                    .filter_map(|l| l.get("uri").and_then(|u| u.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for uri in uris.into_iter().filter(|u| *u != own_uri).take(MAX_OPENED) {
            let path = PathBuf::from(uri.trim_start_matches("file://"));
            let Ok(text) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            if text.len() > MAX_FILE_BYTES {
                continue;
            }
            let did_open = serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id_for_server_path(&path),
                    "version": 1,
                    "text": text,
                }
            });
            if engine
                .send_notification("textDocument/didOpen", did_open)
                .await
                .is_ok()
            {
                opened.push(uri);
            }
        }
    }
    let resp = engine.send_request("textDocument/rename", params).await;
    for uri in &opened {
        let _ = engine
            .send_notification(
                "textDocument/didClose",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await;
    }
    (resp, opened.len())
}

/// LSP `SymbolKind` number for a rust-analyzer symbol kind name.
fn lsp_symbol_kind(kind: &str) -> u64 {
    match kind {
        "Module" | "CrateRoot" => 2,
        "TypeAlias" | "Impl" | "SelfType" => 5,
        "Method" => 6,
        "Field" => 8,
        "Enum" => 10,
        "Trait" => 11,
        "Function" | "Fn" | "Macro" | "ProcMacro" => 12,
        "Const" | "Constant" => 14,
        "Struct" | "Union" => 23,
        "Variant" => 22,
        "TypeParam" | "ConstParam" | "LifetimeParam" => 26,
        _ => 13,
    }
}

fn lsp_range(line: u32, col: u32, end_line: u32, end_col: u32) -> serde_json::Value {
    serde_json::json!({
        "start": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        "end": { "line": end_line.saturating_sub(1), "character": end_col.saturating_sub(1) }
    })
}

fn hierarchy_item_json(item: &prod_code_engine_rust::HierarchyItem) -> serde_json::Value {
    serde_json::json!({
        "name": item.name,
        "kind": lsp_symbol_kind(&item.kind),
        "uri": format!("file://{}", item.path.display()),
        "range": lsp_range(item.line, item.col, item.end_line, item.end_col),
        "selectionRange": lsp_range(item.line, item.col, item.line, item.col + item.name.chars().count() as u32),
    })
}

/// Call hierarchy and implementation queries on the in-memory Rust engine, in LSP shape.
fn hierarchy_query(
    engine: &prod_code_engine_rust::RustEngine,
    method: &str,
    path: &std::path::Path,
    line: u32,
    col: u32,
) -> anyhow::Result<serde_json::Value> {
    Ok(match method {
        "textDocument/prepareCallHierarchy" => serde_json::Value::Array(
            engine
                .prepare_call_hierarchy(path, line, col)?
                .iter()
                .map(hierarchy_item_json)
                .collect(),
        ),
        "callHierarchy/incomingCalls" | "callHierarchy/outgoingCalls" => {
            let incoming = method == "callHierarchy/incomingCalls";
            let edges = if incoming {
                engine.incoming_calls(path, line, col)?
            } else {
                engine.outgoing_calls(path, line, col)?
            };
            serde_json::Value::Array(
                edges
                    .iter()
                    .map(|edge| {
                        let ranges: Vec<_> = edge
                            .call_sites
                            .iter()
                            .map(|(l, c)| lsp_range(*l, *c, *l, *c))
                            .collect();
                        let mut value = serde_json::json!({
                            if incoming { "from" } else { "to" }: hierarchy_item_json(&edge.item),
                            "fromRanges": ranges,
                        });
                        if incoming {
                            value["isTest"] = serde_json::Value::Bool(edge.is_test);
                        }
                        value
                    })
                    .collect(),
            )
        }
        "textDocument/diagnostic" => {
            let items: Vec<serde_json::Value> = engine
                .diagnostics(path)?
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "range": lsp_range(d.line, d.col, d.end_line, d.end_col),
                        "severity": match d.severity.as_str() { "error" => 1, "warning" => 2, "weak" => 4, _ => 3 },
                        "code": d.code,
                        "source": "rust-analyzer",
                        "message": d.message,
                        "tags": if d.unused { vec![1] } else { Vec::<u32>::new() },
                    })
                })
                .collect();
            serde_json::json!({ "kind": "full", "items": items })
        }
        "textDocument/implementation" => serde_json::Value::Array(
            engine
                .goto_implementation(path, line, col)?
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "uri": format!("file://{}", t.path.display()),
                        "range": lsp_range(t.line, t.col, t.line, t.col),
                    })
                })
                .collect(),
        ),
        other => anyhow::bail!("unsupported hierarchy method {other}"),
    })
}

fn workspace_edit_json(outcome: &prod_code_engine_rust::RefactorOutcome) -> serde_json::Value {
    let mut changes = Vec::new();
    for mv in &outcome.moves {
        changes.push(serde_json::json!({
            "kind": "rename",
            "oldUri": format!("file://{}", mv.from.display()),
            "newUri": format!("file://{}", mv.to.display()),
            "options": { "overwrite": false }
        }));
    }
    for created in &outcome.created {
        let uri = format!("file://{}", created.path.display());
        changes.push(
            serde_json::json!({ "kind": "create", "uri": uri, "options": { "overwrite": false } }),
        );
        changes.push(serde_json::json!({
            "textDocument": { "uri": uri, "version": null },
            "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }, "newText": created.new_text } ]
        }));
    }
    for file in &outcome.files {
        changes.push(serde_json::json!({
            "textDocument": { "uri": format!("file://{}", file.path.display()), "version": null },
            "edits": [ {
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": file.old_line_count, "character": 0 } },
                "newText": file.new_text
            } ]
        }));
    }
    serde_json::json!({ "documentChanges": changes })
}

/// Default wall-clock limit for a remote command when the client does not set one.
const EXEC_DEFAULT_TIMEOUT_SECS: u64 = 3600;

/// Size and content hash of every file under `root` the sync layer cares about (build output
/// and VCS internals excluded), for detecting what a remote command changed.
fn snapshot_tree(root: &std::path::Path) -> std::collections::HashMap<String, (u64, u64)> {
    let mut present = Vec::new();
    walk_files(root, root, &mut present);
    present
        .into_iter()
        .filter_map(|(rel, path)| {
            let bytes = std::fs::read(&path).ok()?;
            Some((rel, (bytes.len() as u64, content_hash(&bytes))))
        })
        .collect()
}

/// Files that differ between `before` and the tree now: new/changed ones with content,
/// removed ones as deletions. Files above 5 MiB are ignored.
fn changed_since(
    root: &std::path::Path,
    before: &std::collections::HashMap<String, (u64, u64)>,
) -> Vec<FileDelta> {
    const MAX_PULL_FILE: u64 = 5 * 1024 * 1024;
    let after = snapshot_tree(root);
    let mut out = Vec::new();
    for (rel, stamp) in &after {
        if before.get(rel) == Some(stamp) || stamp.0 > MAX_PULL_FILE {
            continue;
        }
        if let Ok(content) = std::fs::read(root.join(rel)) {
            #[cfg(unix)]
            let is_executable = {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(root.join(rel))
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            };
            #[cfg(not(unix))]
            let is_executable = false;
            out.push(FileDelta {
                relative_path: rel.clone(),
                content: Some(content),
                is_executable,
            });
        }
    }
    for rel in before.keys() {
        if !after.contains_key(rel) {
            out.push(FileDelta {
                relative_path: rel.clone(),
                content: None,
                is_executable: false,
            });
        }
    }
    out.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    out
}

/// Tells a warm engine that the files a command just wrote are its new base, and drops every
/// engine under the workspace when one of them is a project manifest — what a sync does, for a
/// change that did not arrive as a sync.
///
/// A formatter or a generator rewrites files in the workspace copy; the client is sent the new
/// contents and records them as synced, so no later sync ever carries them here. An engine that
/// is not told keeps answering from the text it had before the command, and every position in a
/// file the command moved is off by however many lines it moved — silently, because the file it
/// is asked about is opened fresh by the client and only the *other* files come from its copy.
async fn refresh_engines(
    workspace_manager: &WorkspaceManager,
    server_workspace: &std::path::Path,
    files: &[FileDelta],
) {
    let loaded_rust = workspace_manager
        .get_loaded(server_workspace)
        .await
        .map(|ws| ws.mirrored_rust_engines())
        .unwrap_or_default();
    let mut project_config_changed = false;
    for delta in files {
        project_config_changed |= is_project_config_file(&delta.relative_path);
        let text = match &delta.content {
            Some(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) => Some(text.to_string()),
                Err(_) => continue,
            },
            None => None,
        };
        let target = server_workspace.join(&delta.relative_path);
        for engine_lock in &loaded_rust {
            let mut engine = engine_lock.lock().await;
            if let Err(e) = engine.update_base(&target, text.clone()) {
                tracing::warn!(error = %e, file = %target.display(), "engine update after a command failed");
            }
        }
    }
    if project_config_changed {
        let dropped = workspace_manager.unload_under(server_workspace).await;
        if dropped > 0 {
            tracing::info!(
                dropped,
                "a command changed the project configuration; engines reload on next session"
            );
        }
    }
}

/// Kills a tokio child and everything it spawned (its process group), then the child itself:
/// shadow runs, which let tokio reap their children.
fn kill_exec_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args(["-9", "--", &format!("-{pid}")])
            .status();
    }
    let _ = child.start_kill();
}

/// Runs `req.command` inside the client's server workspace, streaming stdout/stderr chunks to
/// the client and finishing with an `ExecExit`. The child is killed if the client goes away
/// or the timeout elapses.
pub async fn run_exec(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<TcpStream, ProdCodeCodec>,
    req: ExecRequest,
) -> Result<()> {
    use tokio::io::AsyncReadExt;

    let start = Instant::now();
    let workspace = workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let workspace_str = workspace.to_string_lossy().to_string();
    let fail = |error: String| ExecExit {
        exit_code: None,
        duration_ms: 0,
        server_workspace_root: workspace_str.clone(),
        timed_out: false,
        error: Some(error),
        usage: None,
        platform: Some(prod_code_protocol::platform()),
    };
    if !workspace.is_dir() {
        framed
            .send(WireMessage::ExecExit(fail(format!(
                "workspace {workspace_str} is not synced to this gateway"
            ))))
            .await?;
        return Ok(());
    }
    let Some((program, args)) = req.command.split_first() else {
        framed
            .send(WireMessage::ExecExit(fail("empty command".to_string())))
            .await?;
        return Ok(());
    };
    let before = if req.pull_changes {
        let root = workspace.clone();
        tokio::task::spawn_blocking(move || snapshot_tree(&root))
            .await
            .unwrap_or_default()
    } else {
        Default::default()
    };

    let run_dir = match req.subdir.as_deref() {
        Some(sub)
            if !sub.is_empty()
                && !sub.starts_with('/')
                && !sub.split('/').any(|c| c == "..")
                && workspace.join(sub).is_dir() =>
        {
            workspace.join(sub)
        }
        _ => workspace.clone(),
    };
    // A std child, reaped here with `wait4` so its resource use comes back with the exit
    // status (#180); tokio only gets the pipes.
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .current_dir(&run_dir)
        .envs(req.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Own process group, so a timeout or client disconnect can take down the whole tree
    // (cargo -> test binary -> its helpers), not just the direct child.
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            framed
                .send(WireMessage::ExecExit(fail(format!(
                    "failed to start {program}: {e}"
                ))))
                .await?;
            return Ok(());
        }
    };
    tracing::info!(
        workspace = %workspace_str,
        command = %req.command.join(" "),
        "🛠️ [EXEC] started"
    );

    // rapidfire (lock-free MPSC): stdout and stderr readers fan in, the session task drains.
    let (tx, mut rx) = rapidfire::mpsc::bounded::<ExecChunk>(256);
    let mut readers = Vec::new();
    if let Some(mut out) = child
        .stdout
        .take()
        .and_then(|o| tokio::process::ChildStdout::from_std(o).ok())
    {
        let tx = tx.clone();
        readers.push(tokio::spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            while let Ok(n) = out.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if tx
                    .send(ExecChunk {
                        stderr: false,
                        data: Some(buf[..n].to_vec()),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    if let Some(mut err) = child
        .stderr
        .take()
        .and_then(|e| tokio::process::ChildStderr::from_std(e).ok())
    {
        let tx = tx.clone();
        readers.push(tokio::spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            while let Ok(n) = err.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if tx
                    .send(ExecChunk {
                        stderr: true,
                        data: Some(buf[..n].to_vec()),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    drop(tx);
    // Reaped on a blocking thread; the pid stays ours to kill until then.
    let pid = child.id();
    let exited = Arc::new(std::sync::Mutex::new(false));
    let (exit_tx, mut exit_rx) = tokio::sync::oneshot::channel();
    {
        let exited = exited.clone();
        tokio::task::spawn_blocking(move || {
            let _ = exit_tx.send(wait_with_usage(pid as i32, &exited));
            drop(child);
        });
    }

    let timeout = std::time::Duration::from_secs(if req.timeout_secs == 0 {
        EXEC_DEFAULT_TIMEOUT_SECS
    } else {
        req.timeout_secs
    });
    let deadline = tokio::time::Instant::now() + timeout;
    let mut timed_out = false;
    let mut status = None;
    let mut chunks_open = true;
    loop {
        tokio::select! {
            chunk = rx.recv(), if chunks_open => match chunk {
                Ok(chunk) => framed.send(WireMessage::ExecChunk(chunk)).await?,
                Err(_) => chunks_open = false,
            },
            exit = &mut exit_rx, if status.is_none() => {
                status = Some(exit.ok().flatten());
            }
            _ = tokio::time::sleep_until(deadline), if !timed_out && status.is_none() => {
                timed_out = true;
                kill_exec_group(pid, &exited);
            }
            incoming = framed.next(), if status.is_none() => match incoming {
                Some(Ok(WireMessage::Ping)) => framed.send(WireMessage::Pong).await?,
                Some(Ok(WireMessage::Disconnect { .. })) | None => {
                    kill_exec_group(pid, &exited);
                    tracing::info!(workspace = %workspace_str, "🛠️ [EXEC] client left; command killed");
                    return Ok(());
                }
                _ => {}
            }
        }
        if !chunks_open && status.is_some() {
            break;
        }
    }
    for reader in readers {
        let _ = reader.await;
    }
    let (exit_code, usage) = match status.flatten() {
        Some((raw, usage)) => {
            use std::os::unix::process::ExitStatusExt;
            (std::process::ExitStatus::from_raw(raw).code(), Some(usage))
        }
        None => (None, None),
    };
    let duration_ms = start.elapsed().as_millis() as u64;
    tracing::info!(
        workspace = %workspace_str,
        command = %req.command.join(" "),
        exit_code = ?exit_code,
        timed_out,
        duration_ms,
        "🛠️ [EXEC] finished"
    );
    {
        let mut ev = metrics::Event::blank("exec");
        ev.agent = req
            .client_agent
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        ev.host = req
            .client_host
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        ev.workspace = workspace
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ev.command = req.command.join(" ");
        ev.duration_ms = start.elapsed().as_millis() as u64;
        ev.exit_code = exit_code;
        ev.ok = exit_code == Some(0);
        metrics.record(ev);
    }
    if req.pull_changes {
        let root = workspace.clone();
        let files = tokio::task::spawn_blocking(move || changed_since(&root, &before))
            .await
            .unwrap_or_default();
        if !files.is_empty() {
            tracing::info!(
                workspace = %workspace_str,
                files = files.len(),
                "🛠️ [EXEC] sending back files the command changed"
            );
            refresh_engines(workspace_manager, &workspace, &files).await;
            framed
                .send(WireMessage::ExecChanges(ExecChanges { files }))
                .await?;
        }
    }
    framed
        .send(WireMessage::ExecExit(ExecExit {
            exit_code,
            duration_ms,
            server_workspace_root: workspace_str,
            timed_out,
            error: None,
            usage,
            platform: Some(prod_code_protocol::platform()),
        }))
        .await?;
    Ok(())
}

/// What `wait4` says about a finished child: its raw wait status and what it and the
/// descendants it waited for used.
fn wait_with_usage(
    pid: i32,
    exited: &std::sync::Mutex<bool>,
) -> Option<(i32, prod_code_protocol::ExecUsage)> {
    // Wait for the exit without reaping, so that until `exited` is set under the lock the pid
    // can only be this child's, running or a zombie: a kill cannot reach a recycled pid.
    loop {
        // SAFETY: an all-zero `siginfo_t` is a valid value for `waitid` to fill in.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is valid for writes; WNOWAIT leaves the child to be reaped below.
        let got = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if got == 0 {
            break;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return None;
        }
    }
    let mut done = exited.lock().unwrap_or_else(|e| e.into_inner());
    *done = true;
    let mut status: libc::c_int = 0;
    // SAFETY: an all-zero `rusage` is a valid value for `wait4` to fill in.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `status` and `usage` are valid for writes for the duration of the call, and
        // `pid` is a child of this process that nothing else waits for.
        let got = unsafe { libc::wait4(pid, &mut status, 0, &mut usage) };
        if got == pid {
            break;
        }
        if got == -1 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return None;
    }
    let ms = |t: libc::timeval| t.tv_sec as u64 * 1000 + t.tv_usec as u64 / 1000;
    // Linux reports the peak in KiB, macOS in bytes.
    let max_rss_kb = if cfg!(target_os = "macos") {
        usage.ru_maxrss as u64 / 1024
    } else {
        usage.ru_maxrss as u64
    };
    Some((
        status,
        prod_code_protocol::ExecUsage {
            cpu_user_ms: ms(usage.ru_utime),
            cpu_sys_ms: ms(usage.ru_stime),
            max_rss_kb,
        },
    ))
}

/// Kills the process group led by `pid` (the command and everything it spawned), and `pid`
/// itself, unless the child has already exited: then the pid may no longer be its own.
fn kill_exec_group(pid: u32, exited: &std::sync::Mutex<bool>) {
    let done = exited.lock().unwrap_or_else(|e| e.into_inner());
    if *done {
        return;
    }
    let _ = std::process::Command::new("kill")
        .args(["-9", "--", &format!("-{pid}")])
        .status();
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
}

/// Apply batch file synchronization to server workspace storage.
pub async fn apply_sync(
    storage_root: &std::path::Path,
    workspace_manager: &WorkspaceManager,
    req: SyncRequest,
) -> SyncResponse {
    apply_sync_with_metrics(storage_root, workspace_manager, None, req).await
}

pub async fn apply_sync_with_metrics(
    storage_root: &std::path::Path,
    workspace_manager: &WorkspaceManager,
    metrics: Option<&metrics::Metrics>,
    req: SyncRequest,
) -> SyncResponse {
    let start = Instant::now();
    let server_workspace = workspace::resolve_server_workspace(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    // A directory nobody handshook or probed into is either brand new or was reset behind the
    // client's back; a delta landing there must not pass for a complete workspace.
    let workspace_was_fresh = !server_workspace.join(workspace::LAST_USED_MARKER).exists();
    if !workspace_was_fresh {
        workspace::touch_last_used(&server_workspace);
    }
    let folder_name = server_workspace
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("default");
    // A workspace that is already warm in RAM must see the synced files as its new base.
    let loaded_rust = workspace_manager
        .get_loaded(&server_workspace)
        .await
        .map(|ws| ws.mirrored_rust_engines())
        .unwrap_or_default();

    let mut files_updated = 0;
    let mut files_deleted = 0;
    let mut bytes_transferred = 0;

    let mut project_config_changed = false;
    for delta in req.files {
        let target_path = server_workspace.join(&delta.relative_path);
        project_config_changed |= is_project_config_file(&delta.relative_path);
        match delta.content {
            Some(content_bytes) => {
                if let Some(parent) = target_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                bytes_transferred += content_bytes.len();
                if tokio::fs::write(&target_path, &content_bytes).await.is_ok() {
                    files_updated += 1;
                    #[cfg(unix)]
                    if delta.is_executable {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = tokio::fs::set_permissions(
                            &target_path,
                            std::fs::Permissions::from_mode(0o755),
                        )
                        .await;
                    }
                }
                if let Ok(text) = std::str::from_utf8(&content_bytes) {
                    for engine_lock in &loaded_rust {
                        let mut engine = engine_lock.lock().await;
                        if let Err(e) = engine.update_base(&target_path, Some(text.to_string())) {
                            tracing::warn!(error = %e, file = %target_path.display(), "base update failed");
                        }
                    }
                }
            }
            None => {
                if target_path.exists() && tokio::fs::remove_file(&target_path).await.is_ok() {
                    files_deleted += 1;
                    prune_empty_parents(&server_workspace, target_path.parent());
                }
                for engine_lock in &loaded_rust {
                    let mut engine = engine_lock.lock().await;
                    if let Err(e) = engine.update_base(&target_path, None) {
                        tracing::warn!(error = %e, file = %target_path.display(), "base removal failed");
                    }
                }
            }
        }
    }

    // A changed project manifest (tsconfig, package.json, pyproject, CMakeLists, Package.swift,
    // go.mod, Cargo.toml ...) changes what the language server should see: drop the loaded
    // engines so the next session starts them on the new configuration.
    if project_config_changed {
        let dropped = workspace_manager.unload_under(&server_workspace).await;
        if dropped > 0 {
            tracing::info!(
                folder_name,
                dropped,
                "project configuration changed; engines reloaded on next session"
            );
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    tracing::info!(
        folder_name,
        files_updated,
        files_deleted,
        bytes_transferred,
        fresh = workspace_was_fresh,
        duration_ms = %format!("{duration_ms}ms"),
        "⚡ [SYNC] Workspace fast-sync applied"
    );

    if let Some(metrics) = metrics {
        let mut ev = metrics::Event::blank("sync");
        ev.workspace = folder_name.to_string();
        ev.items = (files_updated + files_deleted) as u64;
        ev.bytes = bytes_transferred as u64;
        ev.duration_ms = duration_ms;
        metrics.record(ev);
    }
    SyncResponse {
        files_updated,
        files_deleted,
        bytes_transferred,
        duration_ms,
        server_workspace_root: server_workspace.to_string_lossy().to_string(),
        workspace_was_fresh,
    }
}

pub async fn handle_client(
    socket: TcpStream,
    addr: SocketAddr,
    state: Arc<ServerState>,
) -> Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());

    while let Some(msg_res) = framed.next().await {
        let msg = msg_res?;
        match msg {
            WireMessage::StatusRequest => {
                let status = state.status().await;
                framed.send(WireMessage::StatusResponse(status)).await?;
            }
            WireMessage::Gossip(gossip) => {
                state.absorb_gossip(gossip).await;
                let own = state.own_gossip().await;
                framed.send(WireMessage::Gossip(own)).await?;
            }
            WireMessage::ClusterRequest => {
                let view = state.cluster_view().await;
                framed.send(WireMessage::ClusterResponse(view)).await?;
            }
            WireMessage::PlaceRequest(req) => {
                let resp = state.place(&req).await;
                framed.send(WireMessage::PlaceResponse(resp)).await?;
            }
            WireMessage::MetricsRequest(req) => {
                let node = state.advertise.read().await.clone();
                let resp = state.metrics.summary(&node, req.since_secs);
                framed.send(WireMessage::MetricsResponse(resp)).await?;
            }
            WireMessage::SyncRequest(req) => {
                let workspace = workspace::server_workspace_path(
                    &state.storage_root,
                    &req.client_workspace_root,
                    req.base_workspace_name.as_deref(),
                );
                let touched: Vec<String> =
                    req.files.iter().map(|f| f.relative_path.clone()).collect();
                let resp = apply_sync_with_metrics(
                    &state.storage_root,
                    &state.workspace_manager,
                    Some(&state.metrics),
                    req,
                )
                .await;
                // The files an agent is editing are the ones it validates next: warm them now
                // (#233).
                let synced_rust = priming::synced_rust_files(&workspace, &touched);
                if !synced_rust.is_empty()
                    && let Some(loaded) = state.workspace_manager.get_loaded(&workspace).await
                    && loaded.rust_engine.is_some()
                {
                    // Validation runs on its own engine: that is the one to warm. Loading it
                    // warms the newest files, these among them.
                    let workspace = workspace.clone();
                    tokio::spawn(async move {
                        let view = loaded.validation_view().await;
                        if let Some(engine) = view.rust_engine.clone() {
                            priming::warm_in_background(engine, workspace, synced_rust);
                        }
                    });
                }
                // The search index is kept current by what the sync wrote, so a query never
                // has to walk the tree.
                state.search_indexes.invalidate(&workspace, touched);
                framed.send(WireMessage::SyncResponse(resp)).await?;
            }
            WireMessage::SyncProbeRequest(req) => {
                let resp =
                    apply_sync_probe(&state.storage_root, &state.workspace_manager, req).await;
                framed.send(WireMessage::SyncProbeResponse(resp)).await?;
            }
            WireMessage::ExecRequest(req) => {
                run_exec(
                    &state.storage_root,
                    &state.metrics,
                    &state.workspace_manager,
                    &mut framed,
                    req,
                )
                .await?;
            }
            WireMessage::ShadowRunRequest(req) => {
                shadow::run_shadow(&state, &mut framed, req).await?;
            }
            WireMessage::SearchRequest(req) => {
                let resp = {
                    let state = Arc::clone(&state);
                    tokio::task::spawn_blocking(move || {
                        search::run_search(&state.search_indexes, &state.storage_root, &req)
                    })
                    .await?
                };
                framed.send(WireMessage::SearchResponse(resp)).await?;
            }
            WireMessage::ReadFileRequest(req) => {
                let resp = read_server_file(&state.storage_root, &req);
                framed.send(WireMessage::ReadFileResponse(resp)).await?;
            }
            WireMessage::Ping => {
                framed.send(WireMessage::Pong).await?;
            }
            WireMessage::HandshakeRequest(req) => {
                let session_id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
                state.active_sessions.fetch_add(1, Ordering::Relaxed);

                let client_root_path = PathBuf::from(&req.client_workspace_root);
                let server_workspace = workspace::resolve_server_workspace(
                    &state.storage_root,
                    &req.client_workspace_root,
                    req.base_workspace_name.as_deref(),
                );
                let server_workspace_str = server_workspace.to_string_lossy().to_string();
                workspace::touch_last_used(&server_workspace);

                // A nested project of another language (engine_subpath) gets its own engine
                // rooted there; sync and path translation stay on the checkout root.
                let engine_root = match req.engine_subpath.as_deref() {
                    Some(sub)
                        if !sub.is_empty()
                            && !sub.starts_with('/')
                            && !sub.split('/').any(|c| c == "..")
                            && server_workspace.join(sub).is_dir() =>
                    {
                        server_workspace.join(sub)
                    }
                    Some(sub) if !sub.is_empty() => {
                        tracing::warn!(subpath = sub, "engine_subpath ignored (missing or unsafe)");
                        server_workspace.clone()
                    }
                    _ => server_workspace.clone(),
                };

                let engine_kind =
                    detect::resolve_engine(&engine_root, req.preferred_engine.as_deref());
                let engine = engine_kind.as_str();
                if !state.serves_engine(engine) {
                    let reason = format!(
                        "engine {engine} is not served by this node (--engines {}); pick a node that lists it",
                        state.engine_allowlist.join(",")
                    );
                    tracing::warn!(
                        client_root = %req.client_workspace_root,
                        engine,
                        "refusing handshake: engine not served here"
                    );
                    state.active_sessions.fetch_sub(1, Ordering::Relaxed);
                    framed.send(WireMessage::Disconnect { reason }).await?;
                    return Ok(());
                }
                let translator =
                    PathTranslator::new(&req.client_workspace_root, &server_workspace_str);

                // Attach to shared workspace using leader-follower coalescing
                let shared_ws = state
                    .workspace_manager
                    .get_or_load(&engine_root, engine)
                    .await?;

                let mut session_view = state
                    .workspace_manager
                    .register_session_view(session_id, client_root_path.clone(), shared_ws)
                    .await;
                // A session that only validates proposed texts runs on the workspace's second
                // engine, so its overlays never invalidate the main one (#73).
                if req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_VALIDATION) {
                    session_view.workspace = session_view.accounted.validation_view().await;
                }

                tracing::info!(
                    session_id,
                    client_pid = req.client_pid,
                    client_root = %req.client_workspace_root,
                    server_root = %server_workspace_str,
                    engine_root = %engine_root.display(),
                    engine,
                    is_single_owner = session_view.is_single_owner,
                    "Client session established (Direct-Edit fast path active: {})",
                    session_view.is_single_owner
                );

                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version: PROTOCOL_VERSION,
                        server_pid: state.server_pid,
                        session_id,
                        server_workspace_root: server_workspace_str,
                        detected_engine: engine.to_string(),
                    }))
                    .await?;

                // Session loop for streaming LSP and control messages
                let meta = Arc::new(SessionMeta {
                    session_id,
                    client_name: req.client_name.clone(),
                    agent: req
                        .client_agent
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    host: req
                        .client_host
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    client_addr: addr.to_string(),
                    workspace: engine_root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    engine: engine.to_string(),
                    engine_root: engine_root.clone(),
                    metrics: Arc::clone(&state.metrics),
                });
                let session_res = run_session_loop(framed, &translator, &session_view, meta).await;

                if let Some(engine_lock) = &session_view.workspace.rust_engine {
                    let mut engine = engine_lock.lock().await;
                    if let Err(e) = engine.clear_session(session_id) {
                        tracing::warn!(error = %e, session_id, "failed to drop session overlays");
                    }
                }

                state
                    .workspace_manager
                    .unregister_session_view(&session_view)
                    .await;
                state.active_sessions.fetch_sub(1, Ordering::Relaxed);

                tracing::info!(session_id, "Client session retired: {:?}", session_res);
                return session_res;
            }
            WireMessage::Disconnect { reason } => {
                tracing::info!(%addr, reason, "Client disconnected cleanly");
                break;
            }
            other => {
                tracing::warn!(%addr, ?other, "Unexpected message before handshake");
            }
        }
    }

    Ok(())
}

async fn run_session_loop(
    framed: Framed<TcpStream, ProdCodeCodec>,
    translator: &PathTranslator,
    view: &SessionView,
    meta: Arc<SessionMeta>,
) -> Result<()> {
    let (mut socket_tx, mut socket_rx) = framed.split();
    // rapidfire MPSC: every engine task sends, one writer drains in batches and flushes the
    // socket once per batch.
    let (out_tx, mut out_rx) = rapidfire::mpsc::bounded::<WireMessage>(4096);

    // Requests in flight, keyed by JSON-RPC id, so every answer — whichever engine produced
    // it — becomes one metrics event with its duration.
    let pending: Arc<tokio::sync::Mutex<std::collections::HashMap<String, PendingRequest>>> =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let pending_writer = Arc::clone(&pending);
    let meta_writer = Arc::clone(&meta);
    let writer_handle = tokio::spawn(async move {
        let mut batch: Vec<WireMessage> = Vec::with_capacity(64);
        'writer: while out_rx.recv_many(&mut batch, 64).await.is_ok() {
            for msg in batch.drain(..) {
                if let WireMessage::LspPayload(ref raw) = msg
                    && let Ok(val) = serde_json::from_str::<serde_json::Value>(raw)
                    && let Some(id) = val.get("id").filter(|i| !i.is_null())
                    && val.get("method").is_none()
                {
                    let key = id.to_string();
                    if let Some(req) = pending_writer.lock().await.remove(&key) {
                        let mut ev = metrics::Event::blank("lsp");
                        ev.session_id = meta_writer.session_id;
                        ev.client_name = meta_writer.client_name.clone();
                        ev.agent = meta_writer.agent.clone();
                        ev.host = meta_writer.host.clone();
                        ev.client_addr = meta_writer.client_addr.clone();
                        ev.workspace = meta_writer.workspace.clone();
                        ev.engine = meta_writer.engine.clone();
                        ev.method = req.method;
                        ev.file = req.file;
                        ev.line = req.line;
                        ev.col = req.col;
                        ev.duration_ms = req.start.elapsed().as_millis() as u64;
                        ev.ok = val.get("error").is_none();
                        ev.items = val
                            .get("result")
                            .map(|r| match r {
                                serde_json::Value::Array(a) => a.len() as u64,
                                serde_json::Value::Null => 0,
                                _ => 1,
                            })
                            .unwrap_or(0);
                        meta_writer.metrics.record(ev);
                    }
                }
                if socket_tx.feed(msg).await.is_err() {
                    break 'writer;
                }
            }
            if socket_tx.flush().await.is_err() {
                break;
            }
        }
    });

    let mut backend_rx = if let Some(ref go) = view.workspace.go_engine {
        Some(go.subscribe())
    } else if let Some(ref generic_eng) = view.workspace.generic_engine {
        Some(generic_eng.subscribe())
    } else {
        view.workspace.backend.as_ref().map(|b| b.subscribe())
    };

    loop {
        tokio::select! {
            client_msg_res = socket_rx.next() => {
                match on_client_message(client_msg_res, &out_tx, translator, view, &meta, &pending).await {
                    Flow::Next => continue,
                    Flow::Stop => break,
                }
            }

            backend_msg = async {
                if let Some(ref mut rx) = backend_rx {
                    rx.recv().await
                } else {
                    futures_util::future::pending::<Result<String, tokio::sync::broadcast::error::RecvError>>().await
                }
            } => {
                match backend_msg {
                    Ok(server_lsp) => {
                        let client_lsp = translator.translate_lsp_to_client(&server_lsp);
                        if out_tx.send(WireMessage::LspPayload(client_lsp)).await.is_err() {
                            tracing::error!("Failed to send LSP message to client channel");
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "Session backend receiver lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::warn!("Backend worker broadcast closed");
                        break;
                    }
                }
            }
        }
    }
    drop(out_tx);
    let _ = writer_handle.await;
    Ok(())
}

/// What the session loop does once a client message has been handled.
enum Flow {
    /// Wait for the next message.
    Next,
    /// The client is gone or asked to disconnect: end the session.
    Stop,
}

/// One message from the client: an LSP payload answered by the in-memory engine or forwarded
/// to the backend, a sync, a status request or a disconnect.
///
/// It lived inside the session loop's `tokio::select!`, where it was 1,400 lines of macro
/// input: rust-analyzer offers no refactoring inside a macro call, and every validation of the
/// file inferred it as one body (#86). Out here it is ordinary code.
async fn on_client_message(
    client_msg_res: Option<std::result::Result<WireMessage, std::io::Error>>,
    out_tx: &rapidfire::mpsc::Sender<WireMessage>,
    translator: &PathTranslator,
    view: &SessionView,
    meta: &Arc<SessionMeta>,
    pending: &Arc<tokio::sync::Mutex<std::collections::HashMap<String, PendingRequest>>>,
) -> Flow {
    match client_msg_res {
        Some(Ok(WireMessage::Ping)) => {
            let _ = out_tx.send(WireMessage::Pong).await;
        }
        Some(Ok(WireMessage::LspPayload(raw_client_lsp))) => {
            let server_lsp = translator.translate_lsp_to_server(&raw_client_lsp);
            tracing::debug!(
                payload_len = server_lsp.len(),
                single_owner = view.is_single_owner,
                "Processing incoming LSP message"
            );

            // Inspect LSP message structure
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&server_lsp) {
                let method = val.get("method").and_then(|m| m.as_str());
                let id = val.get("id").cloned();
                if let (Some(m), Some(id_val)) = (method, &id)
                    && !id_val.is_null()
                    && m != "initialize"
                {
                    let params = val.get("params");
                    let uri = params
                        .and_then(|p| {
                            p.get("textDocument")
                                .and_then(|t| t.get("uri"))
                                .or_else(|| p.get("item").and_then(|i| i.get("uri")))
                        })
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    let file = std::path::Path::new(uri.trim_start_matches("file://"))
                        .strip_prefix(&meta.engine_root)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| uri.trim_start_matches("file://").to_string());
                    let pos = params.and_then(|p| {
                        p.get("position")
                            .or_else(|| p.get("range").and_then(|r| r.get("start")))
                    });
                    pending.lock().await.insert(
                        id_val.to_string(),
                        PendingRequest {
                            method: m.to_string(),
                            file,
                            line: pos
                                .and_then(|p| p.get("line"))
                                .and_then(|l| l.as_u64())
                                .unwrap_or(0) as u32
                                + 1,
                            col: pos
                                .and_then(|p| p.get("character"))
                                .and_then(|c| c.as_u64())
                                .unwrap_or(0) as u32
                                + 1,
                            start: Instant::now(),
                        },
                    );
                }

                // 1. Intercept "initialize": reply immediately with cached server capabilities
                if method == Some("initialize") {
                    let req_id = id.unwrap_or(serde_json::json!(1));
                    let caps = if let Some(ref go) = view.workspace.go_engine {
                        go.capabilities.read().await.clone()
                    } else if let Some(ref generic_eng) = view.workspace.generic_engine {
                        generic_eng.capabilities.read().await.clone()
                    } else if let Some(ref backend) = view.workspace.backend {
                        backend.capabilities.read().await.clone()
                    } else {
                        None
                    };
                    let init_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": {
                            "capabilities": caps.unwrap_or_else(|| serde_json::json!({
                                "textDocumentSync": 1,
                                "hoverProvider": true,
                                "definitionProvider": true,
                                "referencesProvider": true,
                                "documentSymbolProvider": true,
                                "workspaceSymbolProvider": true
                            })),
                            "serverInfo": {
                                "name": "prod-code-rci",
                                "version": "0.1.0"
                            }
                        }
                    });
                    let client_resp = translator.translate_lsp_to_client(&init_resp.to_string());
                    let _ = out_tx.send(WireMessage::LspPayload(client_resp)).await;
                    return Flow::Next;
                }

                // 2. Intercept "initialized": backend already initialized, consume without forwarding
                if method == Some("initialized") {
                    return Flow::Next;
                }

                // 3. Intercept "shutdown": reply cleanly
                if method == Some("shutdown") {
                    let req_id = id.unwrap_or(serde_json::json!(1));
                    let shutdown_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": null
                    });
                    let _ = out_tx
                        .send(WireMessage::LspPayload(shutdown_resp.to_string()))
                        .await;
                    return Flow::Next;
                }

                // 4. In-Memory RustEngine multi-core fast path: hover, definition, references, documentSymbol
                if let Some(ref engine_lock) = view.workspace.rust_engine {
                    match method {
                        Some("textDocument/hover") => {
                            if let Some(params) = val.get("params") {
                                lsp_hover(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/definition") => {
                            if let Some(params) = val.get("params") {
                                lsp_definition(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/references") => {
                            if let Some(params) = val.get("params") {
                                lsp_references(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/documentSymbol") => {
                            if let Some(params) = val.get("params") {
                                lsp_document_symbol(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("workspace/symbol") => {
                            if let Some(params) = val.get("params") {
                                lsp_workspace_symbol(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("prodCode/assists") | Some("prodCode/applyAssist") => {
                            if let Some(params) = val.get("params") {
                                lsp_assists(
                                    out_tx,
                                    translator,
                                    view,
                                    method,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("prodCode/safeDelete") => {
                            if let Some(params) = val.get("params") {
                                lsp_safe_delete(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some(
                            hm @ ("textDocument/prepareCallHierarchy"
                            | "callHierarchy/incomingCalls"
                            | "callHierarchy/outgoingCalls"
                            | "textDocument/implementation"
                            | "textDocument/diagnostic"),
                        ) => {
                            if let Some(params) = val.get("params") {
                                lsp_call_hierarchy(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    hm,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/rename") => {
                            if let Some(params) = val.get("params") {
                                lsp_rename(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("prodCode/structuralReplace") => {
                            if let Some(params) = val.get("params") {
                                lsp_structural_replace(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/didOpen") => {
                            if let Some(params) = val.get("params") {
                                let uri = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let file_path = PathBuf::from(uri.trim_start_matches("file://"));
                                if let Some(text) = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("text"))
                                    .and_then(|t| t.as_str())
                                {
                                    let edit_start = Instant::now();
                                    let text_len = text.len();
                                    {
                                        let mut engine = engine_lock.lock().await;
                                        if let Err(e) = engine.set_session_overlay(
                                            view.session_id,
                                            &file_path,
                                            Some(text.to_string()),
                                        ) {
                                            tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                        }
                                    }
                                    let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                    tracing::info!(
                                        session = view.session_id,
                                        file = %file_path.display(),
                                        bytes = text_len,
                                        duration_ms = format!("{:.2}ms", ms),
                                        "📝 [OVERLAY] didOpen recorded as session buffer in Salsa DB"
                                    );
                                }
                            }
                        }
                        Some("textDocument/didChange") => {
                            if let Some(params) = val.get("params") {
                                let uri = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let file_path = PathBuf::from(uri.trim_start_matches("file://"));
                                let first = params
                                    .get("contentChanges")
                                    .and_then(|c| c.as_array())
                                    .and_then(|arr| arr.first())
                                    .and_then(|c| c.get("text"))
                                    .and_then(|t| t.as_str());
                                if let Some(text) = first {
                                    let edit_start = Instant::now();
                                    let text_len = text.len();
                                    {
                                        let mut engine = engine_lock.lock().await;
                                        if let Err(e) = engine.set_session_overlay(
                                            view.session_id,
                                            &file_path,
                                            Some(text.to_string()),
                                        ) {
                                            tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                        }
                                    }
                                    let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                    tracing::info!(
                                        session = view.session_id,
                                        file = %file_path.display(),
                                        bytes = text_len,
                                        duration_ms = format!("{:.2}ms", ms),
                                        "📝 [OVERLAY] didChange recorded as session buffer in Salsa DB"
                                    );
                                }
                            }
                        }
                        Some("textDocument/didClose") => {
                            if let Some(params) = val.get("params") {
                                let uri = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let file_path = PathBuf::from(uri.trim_start_matches("file://"));
                                let mut engine = engine_lock.lock().await;
                                if let Err(e) =
                                    engine.clear_session_overlay(view.session_id, &file_path)
                                {
                                    tracing::warn!(error = %e, file = %file_path.display(), "session overlay close failed");
                                }
                            }
                            return Flow::Next;
                        }
                        _ => {}
                    }
                }

                // 5. Fallback handling for textDocument/didOpen vs didChange on backend worker
                if let (Some("textDocument/didOpen"), Some(backend)) =
                    (method, &view.workspace.backend)
                {
                    let uri = val
                        .get("params")
                        .and_then(|p| p.get("textDocument"))
                        .and_then(|td| td.get("uri"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    let is_open = backend.open_files.read().await.contains(uri);
                    if is_open {
                        let text = val
                            .get("params")
                            .and_then(|p| p.get("textDocument"))
                            .and_then(|td| td.get("text"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        let version = val
                            .get("params")
                            .and_then(|p| p.get("textDocument"))
                            .and_then(|td| td.get("version"))
                            .and_then(|v| v.as_i64())
                            .unwrap_or(2);
                        let did_change = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "textDocument/didChange",
                            "params": {
                                "textDocument": {
                                    "uri": uri,
                                    "version": version
                                },
                                "contentChanges": [
                                    { "text": text }
                                ]
                            }
                        });
                        let _ = backend.send_lsp(&did_change.to_string()).await;
                        return Flow::Next;
                    } else {
                        backend.open_files.write().await.insert(uri.to_string());
                    }
                }

                // 5a'. Code actions on managed language servers: the Rust-style
                // prodCode/assists | applyAssist requests become LSP codeAction.
                if let (Some(pm @ ("prodCode/assists" | "prodCode/applyAssist")), Some(req_id)) =
                    (method, &id)
                    && (view.workspace.go_engine.is_some()
                        || view.workspace.generic_engine.is_some())
                {
                    let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                    let go = view.workspace.go_engine.clone();
                    let generic = view.workspace.generic_engine.clone();
                    let out_tx_task = out_tx.clone();
                    let translator_task = translator.clone();
                    let r_id = req_id.clone();
                    let session_id = view.session_id;
                    let method_name = pm.to_string();
                    let start = Instant::now();
                    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                    tokio::task::spawn(async move {
                        let engine = match (&go, &generic) {
                            (_, Some(g)) => ManagedLsp::Generic(g),
                            (Some(g), None) => ManagedLsp::Go(g),
                            (None, None) => unreachable!("guarded above"),
                        };
                        let outcome = lsp_code_actions(&engine, &method_name, params).await;
                        let ms = start.elapsed().as_secs_f64() * 1000.0;
                        let resp = match outcome {
                            Ok(result) => {
                                tracing::info!(session = session_id, method = %method_name, duration_ms = format!("{ms:.2}ms"), "✅ [LSP DONE] code actions");
                                serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "result": result })
                            }
                            Err(err) => {
                                tracing::info!(session = session_id, method = %method_name, error = %err, "🚫 [LSP REFUSED] code actions");
                                serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "error": { "code": -32602, "message": err.to_string() } })
                            }
                        };
                        let client_resp =
                            translator_task.translate_lsp_to_client(&resp.to_string());
                        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                    });
                    return Flow::Next;
                }

                // 5b. Supervised GoEngine fast path
                if let Some(ref go) = view.workspace.go_engine {
                    match method {
                        Some("textDocument/didOpen") => {
                            let uri = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");
                            let text = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("text"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            let _ = go.did_open(uri, text).await;
                            return Flow::Next;
                        }
                        Some("textDocument/didChange") => {
                            let uri = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");
                            let version = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("version"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(1) as i32;
                            let text = val
                                .get("params")
                                .and_then(|p| p.get("contentChanges"))
                                .and_then(|c| c.as_array())
                                .and_then(|a| a.first())
                                .and_then(|ch| ch.get("text"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            let _ = go.did_change(uri, text, version).await;
                            return Flow::Next;
                        }
                        Some("textDocument/didClose") => {
                            let uri = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");
                            let _ = go.did_close(uri).await;
                            return Flow::Next;
                        }
                        Some(m) if id.is_some() => {
                            let req_id_log = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                            let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                            TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                            let start = Instant::now();

                            tracing::info!(
                                req = req_id_log,
                                session = view.session_id,
                                method = m,
                                in_flight,
                                "🚀 [LSP START] dispatching to GoEngine"
                            );

                            let params =
                                val.get("params").cloned().unwrap_or(serde_json::json!({}));
                            let out_tx_task = out_tx.clone();
                            let go_clone = Arc::clone(go);
                            let translator_task = translator.clone();
                            let session_id = view.session_id;
                            let method_str = m.to_string();
                            let req_id = id.clone();

                            tokio::task::spawn(async move {
                                let resp_res = go_clone.send_request(&method_str, params).await;
                                let duration = start.elapsed();
                                let duration_ms = duration.as_secs_f64() * 1000.0;
                                let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

                                if duration_ms > 200.0 {
                                    SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                    tracing::warn!(
                                        req = req_id_log,
                                        session = session_id,
                                        method = %method_str,
                                        duration_ms = %format!("{:.2}ms", duration_ms),
                                        in_flight = remaining,
                                        "⚠️ [LSP SLOW >200ms] GoEngine query exceeded threshold"
                                    );
                                } else {
                                    tracing::info!(
                                        req = req_id_log,
                                        session = session_id,
                                        method = %method_str,
                                        duration_ms = %format!("{:.2}ms", duration_ms),
                                        in_flight = remaining,
                                        "✅ [LSP DONE] GoEngine query complete"
                                    );
                                }

                                match resp_res {
                                    Ok(mut resp) => {
                                        if let Some(ref r_id) = req_id {
                                            resp["id"] = r_id.clone();
                                        }
                                        let client_resp = translator_task
                                            .translate_lsp_to_client(&resp.to_string());
                                        let _ = out_tx_task
                                            .send(WireMessage::LspPayload(client_resp))
                                            .await;
                                    }
                                    Err(err) => {
                                        let err_resp = serde_json::json!({
                                            "jsonrpc": "2.0",
                                            "id": req_id,
                                            "error": { "code": -32603, "message": err.to_string() }
                                        });
                                        let _ = out_tx_task
                                            .send(WireMessage::LspPayload(err_resp.to_string()))
                                            .await;
                                    }
                                }
                            });
                            return Flow::Next;
                        }
                        Some(m) => {
                            let params =
                                val.get("params").cloned().unwrap_or(serde_json::json!({}));
                            let _ = go.send_notification(m, params).await;
                            return Flow::Next;
                        }
                        None => {}
                    }
                }

                // 5c. Supervised GenericLspEngine fast path
                if let Some(ref generic_eng) = view.workspace.generic_engine {
                    if let (Some("textDocument/rename"), Some(req_id)) = (method, &id) {
                        // Servers such as pyright only rename inside open documents:
                        // open every file that references the symbol first.
                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                        let engine = Arc::clone(generic_eng);
                        let out_tx_task = out_tx.clone();
                        let translator_task = translator.clone();
                        let r_id = req_id.clone();
                        let session_id = view.session_id;
                        let start = Instant::now();
                        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                        tokio::task::spawn(async move {
                            let (resp, opened) = rename_with_references_open(&engine, params).await;
                            tracing::info!(
                                session = session_id,
                                opened,
                                duration_ms =
                                    format!("{:.2}ms", start.elapsed().as_secs_f64() * 1000.0),
                                "✅ [LSP DONE] generic rename"
                            );
                            let resp = match resp {
                                Ok(mut resp) => {
                                    resp["id"] = r_id;
                                    resp
                                }
                                Err(err) => {
                                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "error": { "code": -32603, "message": err.to_string() } })
                                }
                            };
                            let client_resp =
                                translator_task.translate_lsp_to_client(&resp.to_string());
                            let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                        });
                        return Flow::Next;
                    }
                    if let (Some("textDocument/diagnostic"), Some(req_id)) = (method, &id) {
                        // Servers without pull diagnostics answer from what they
                        // published for the document.
                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                        let engine = Arc::clone(generic_eng);
                        let out_tx_task = out_tx.clone();
                        let translator_task = translator.clone();
                        let r_id = req_id.clone();
                        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                        tokio::task::spawn(async move {
                            let resp = if engine.has_pull_diagnostics().await {
                                match engine.send_request("textDocument/diagnostic", params).await {
                                    Ok(mut resp) => {
                                        resp["id"] = r_id;
                                        resp
                                    }
                                    Err(err) => {
                                        serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "error": { "code": -32603, "message": err.to_string() } })
                                    }
                                }
                            } else {
                                let uri = params
                                    .get("textDocument")
                                    .and_then(|t| t.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let items =
                                    ManagedLsp::Generic(&engine).diagnostics_for(&uri).await;
                                serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "result": { "kind": "full", "items": items } })
                            };
                            let client_resp =
                                translator_task.translate_lsp_to_client(&resp.to_string());
                            let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                        });
                        return Flow::Next;
                    }
                    if let (Some(m), Some(req_id)) = (method, &id) {
                        let req_id_log = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                        let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                        let start = Instant::now();

                        tracing::info!(
                            req = req_id_log,
                            session = view.session_id,
                            method = m,
                            in_flight,
                            "🚀 [LSP START] dispatching to GenericLspEngine"
                        );

                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                        let out_tx_task = out_tx.clone();
                        let generic_eng_clone = Arc::clone(generic_eng);
                        let translator_task = translator.clone();
                        let session_id = view.session_id;
                        let method_str = m.to_string();
                        let r_id = req_id.clone();

                        tokio::task::spawn(async move {
                            let resp_res =
                                generic_eng_clone.send_request(&method_str, params).await;
                            let duration = start.elapsed();
                            let duration_ms = duration.as_secs_f64() * 1000.0;
                            let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

                            if duration_ms > 200.0 {
                                SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                tracing::warn!(
                                    req = req_id_log,
                                    session = session_id,
                                    method = %method_str,
                                    duration_ms = %format!("{:.2}ms", duration_ms),
                                    in_flight = remaining,
                                    "⚠️ [LSP SLOW >200ms] GenericLspEngine query exceeded threshold"
                                );
                            } else {
                                tracing::info!(
                                    req = req_id_log,
                                    session = session_id,
                                    method = %method_str,
                                    duration_ms = %format!("{:.2}ms", duration_ms),
                                    in_flight = remaining,
                                    "✅ [LSP DONE] GenericLspEngine query complete"
                                );
                            }

                            match resp_res {
                                Ok(mut resp) => {
                                    resp["id"] = r_id;
                                    let client_resp =
                                        translator_task.translate_lsp_to_client(&resp.to_string());
                                    let _ = out_tx_task
                                        .send(WireMessage::LspPayload(client_resp))
                                        .await;
                                }
                                Err(err) => {
                                    let err_resp = serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": r_id,
                                        "error": { "code": -32603, "message": err.to_string() }
                                    });
                                    let _ = out_tx_task
                                        .send(WireMessage::LspPayload(err_resp.to_string()))
                                        .await;
                                }
                            }
                        });
                        return Flow::Next;
                    } else if let Some(m) = method {
                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                        let _ = generic_eng.send_notification(m, params).await;
                        return Flow::Next;
                    }
                }

                // 6. Handle "textDocument/didClose"
                if let (Some("textDocument/didClose"), Some(backend)) =
                    (method, &view.workspace.backend)
                {
                    let uri = val
                        .get("params")
                        .and_then(|p| p.get("textDocument"))
                        .and_then(|td| td.get("uri"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    backend.open_files.write().await.remove(uri);
                }

                // 7. If no backend is attached and client expects a response, return empty result
                if let (Some(req_id), None, None, None, None) = (
                    id,
                    &view.workspace.backend,
                    &view.workspace.rust_engine,
                    &view.workspace.go_engine,
                    &view.workspace.generic_engine,
                ) {
                    let empty_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": null
                    });
                    let _ = out_tx
                        .send(WireMessage::LspPayload(empty_resp.to_string()))
                        .await;
                    return Flow::Next;
                }
            }

            if let Some(backend) = &view.workspace.backend {
                let _ = backend.send_lsp(&server_lsp).await.inspect_err(|e| {
                    tracing::error!(error = %e, "Failed to forward LSP to backend worker");
                });
            }
        }
        Some(Ok(WireMessage::SyncRequest(req))) => {
            let start = Instant::now();
            let mut files_updated = 0;
            let mut files_deleted = 0;
            let mut bytes_transferred = 0;

            for delta in &req.files {
                let target_path = view.workspace.root.join(&delta.relative_path);
                match &delta.content {
                    Some(content_bytes) => {
                        if let Some(parent) = target_path.parent() {
                            let _ = tokio::fs::create_dir_all(parent).await;
                        }
                        bytes_transferred += content_bytes.len();
                        if tokio::fs::write(&target_path, content_bytes).await.is_ok() {
                            files_updated += 1;
                            #[cfg(unix)]
                            if delta.is_executable {
                                use std::os::unix::fs::PermissionsExt;
                                let _ = tokio::fs::set_permissions(
                                    &target_path,
                                    std::fs::Permissions::from_mode(0o755),
                                )
                                .await;
                            }
                        }
                        // The workspace is this worktree's own: synced files are its
                        // new base, visible to every session except one that still
                        // holds an unsaved buffer for the same path.
                        if let Ok(text) = std::str::from_utf8(content_bytes) {
                            for engine_lock in view.workspace.mirrored_rust_engines() {
                                let mut engine = engine_lock.lock().await;
                                if let Err(e) =
                                    engine.update_base(&target_path, Some(text.to_string()))
                                {
                                    tracing::warn!(error = %e, file = %target_path.display(), "base update failed");
                                }
                            }
                        }
                    }
                    None => {
                        if target_path.exists()
                            && tokio::fs::remove_file(&target_path).await.is_ok()
                        {
                            files_deleted += 1;
                            prune_empty_parents(&view.workspace.root, target_path.parent());
                        }
                        for engine_lock in view.workspace.mirrored_rust_engines() {
                            let mut engine = engine_lock.lock().await;
                            if let Err(e) = engine.update_base(&target_path, None) {
                                tracing::warn!(error = %e, file = %target_path.display(), "base removal failed");
                            }
                        }
                    }
                }
            }

            if req.clean_others
                && let Some(engine_lock) = &view.workspace.rust_engine
            {
                // The request is the session's complete dirty set: any other
                // overlay this session still holds is stale (reverted or committed).
                let keep: Vec<PathBuf> = req
                    .files
                    .iter()
                    .map(|delta| view.workspace.root.join(&delta.relative_path))
                    .collect();
                let mut engine = engine_lock.lock().await;
                match engine.retain_session_overlays(view.session_id, &keep) {
                    Ok(dropped) if dropped > 0 => tracing::info!(
                        session = view.session_id,
                        dropped,
                        "🧹 [OVERLAY] dropped stale session buffers after full dirty sync"
                    ),
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, session = view.session_id, "failed to drop stale session buffers")
                    }
                }
            }

            let duration_ms = start.elapsed().as_millis() as u64;
            let _ = out_tx
                .send(WireMessage::SyncResponse(SyncResponse {
                    files_updated,
                    files_deleted,
                    bytes_transferred,
                    duration_ms,
                    server_workspace_root: view.workspace.root.to_string_lossy().to_string(),
                    workspace_was_fresh: false,
                }))
                .await;
        }
        Some(Ok(WireMessage::Disconnect { reason })) => {
            tracing::info!(reason, "Client terminated session");
            return Flow::Stop;
        }
        Some(Ok(WireMessage::StatusRequest)) => {
            let _ = out_tx
                .send(WireMessage::StatusResponse(StatusResponse {
                    server_pid: std::process::id(),
                    uptime_seconds: 0,
                    active_sessions: 1,
                    loaded_workspaces: 1,
                    detected_engines: vec![view.workspace.engine.clone()],
                    memory_rss_bytes: memory::get_process_rss_bytes(),
                    total_queries: TOTAL_QUERIES.load(Ordering::Relaxed),
                    active_queries: ACTIVE_QUERIES.load(Ordering::Relaxed),
                    load_average_millis: memory::load_average_1m().map(|l| (l * 1000.0) as u32),
                    cpu_count: std::thread::available_parallelism().ok().map(|n| n.get()),
                }))
                .await;
        }
        Some(Err(e)) => {
            tracing::error!(error = %e, "TCP frame decode error");
            return Flow::Stop;
        }
        None => {
            tracing::info!("Client disconnected");
            return Flow::Stop;
        }
        _ => {}
    }
    Flow::Next
}

fn lsp_call_hierarchy(
    out_tx: &rapidfire::Sender<WireMessage>,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    hm: &str,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    // Call-hierarchy follow-ups carry the item; the others a text document position.
    let (uri, position) = match params.get("item") {
        Some(item) => (
            item.get("uri").and_then(|u| u.as_str()).unwrap_or(""),
            item.get("selectionRange").and_then(|r| r.get("start")),
        ),
        None => (
            params
                .get("textDocument")
                .and_then(|td| td.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or(""),
            params.get("position"),
        ),
    };
    let line = position
        .and_then(|p| p.get("line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0) as u32;
    let col = position
        .and_then(|p| p.get("character"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;
    let file_path = PathBuf::from(uri.trim_start_matches("file://"));
    let method_name = hm.to_string();

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(req = req_num, session = view.session_id, method = %method_name, file = %file_path.display(), pos = format!("{}:{}", line + 1, col + 1), in_flight, "🚀 [LSP START]");

    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;

    tokio::task::spawn(async move {
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            let m = method_name.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = engine.activate_session(session_id) {
                    tracing::warn!(error = %e, session = session_id, "session view activation failed");
                }
                hierarchy_query(&engine, &m, &fp_clone, line + 1, col + 1)
            })
            .await
            .unwrap_or_else(|e| {
                // A panic in the analyzer while checking a file is the analyzer's, not the
                // request's: the file simply could not be checked. As an error the whole
                // validation failed; as one diagnostic it is reported and the rest goes on (#94).
                if e.is_panic() && method_name == "textDocument/diagnostic" {
                    Ok(analyzer_panic_report(&panic_message(e.into_panic())))
                } else {
                    Err(anyhow::anyhow!("query task failed: {e}"))
                }
            })
        };
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let resp = match outcome {
            Ok(result) => {
                tracing::info!(req = req_num, session = session_id, method = %method_name, duration_ms = format!("{:.2}ms", ms), items = result.as_array().map(|a| a.len()).unwrap_or(0), in_flight = remaining, "✅ [LSP DONE]");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": result })
            }
            Err(e) => {
                tracing::warn!(req = req_num, session = session_id, method = %method_name, error = %e, "query failed");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": e.to_string() } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

/// Code of the diagnostic that stands for a file the analyzer panicked on.
pub const ANALYZER_PANIC: &str = prod_code_protocol::ANALYZER_PANIC_CODE;

/// The text of a panic payload, when it is one of the two shapes `panic!` produces.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "no message".to_string())
}

/// The diagnostics report for a file the analyzer panicked on: one error at its top that says
/// nothing in it was checked, and what check remains.
fn analyzer_panic_report(message: &str) -> serde_json::Value {
    let message = message.trim().trim_end_matches('.');
    serde_json::json!({ "kind": "full", "items": [ {
        "range": lsp_range(1, 1, 1, 1),
        "severity": 1,
        "code": ANALYZER_PANIC,
        "source": "prod-code",
        "message": format!(
            "rust-analyzer panicked while checking this file, so nothing in it was checked: \
             {message}. The compiler is the check that remains (`verify: \"compile\"`, or \
             `code_check`)."
        ),
    } ] })
}

fn lsp_safe_delete(
    out_tx: &rapidfire::Sender<WireMessage>,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let uri = params
        .get("textDocument")
        .and_then(|td| td.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    let line = params
        .get("position")
        .and_then(|p| p.get("line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0) as u32;
    let col = params
        .get("position")
        .and_then(|p| p.get("character"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;
    let file_path = PathBuf::from(uri.trim_start_matches("file://"));
    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(req = req_num, session = view.session_id, method = "prodCode/safeDelete", file = %file_path.display(), pos = format!("{}:{}", line + 1, col + 1), in_flight, "🚀 [LSP START]");
    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    tokio::task::spawn(async move {
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if let Err(e) = engine.activate_session(session_id) {
                    tracing::warn!(error = %e, session = session_id, "session view activation failed");
                }
                engine.safe_delete(&fp_clone, line + 1, col + 1)
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("safe delete task failed: {e}")))
        };
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let resp = match outcome {
            Ok(Ok(outcome)) => {
                tracing::info!(
                    req = req_num,
                    session = session_id,
                    method = "prodCode/safeDelete",
                    duration_ms = format!("{:.2}ms", ms),
                    in_flight = remaining,
                    "✅ [LSP DONE]"
                );
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": workspace_edit_json(&outcome) })
            }
            Ok(Err(refused)) => {
                tracing::info!(
                    req = req_num,
                    session = session_id,
                    method = "prodCode/safeDelete",
                    "🚫 [LSP REFUSED]"
                );
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32602, "message": refused } })
            }
            Err(e) => {
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": e.to_string() } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

fn lsp_structural_replace(
    out_tx: &rapidfire::Sender<WireMessage>,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let uri = params
        .get("textDocument")
        .and_then(|td| td.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    let line = params
        .get("position")
        .and_then(|p| p.get("line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0) as u32;
    let col = params
        .get("position")
        .and_then(|p| p.get("character"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;
    let rule = params
        .get("rule")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    let scope = params
        .get("scope")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| PathBuf::from(s.trim_start_matches("file://")));
    let file_path = PathBuf::from(uri.trim_start_matches("file://"));

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "prodCode/structuralReplace",
        file = %file_path.display(),
        pos = format!("{}:{}", line + 1, col + 1),
        rule = %rule,
        in_flight,
        "🚀 [LSP START]"
    );

    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;

    tokio::task::spawn(async move {
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if let Err(e) = engine.activate_session(session_id) {
                    tracing::warn!(error = %e, session = session_id, "session view activation failed");
                }
                engine.structural_replace(&rule, &fp_clone, line + 1, col + 1, scope.as_deref())
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("codemod task failed: {e}")))
        };
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let resp = match outcome {
            Ok(Ok(outcome)) => {
                tracing::info!(
                    req = req_num,
                    session = session_id,
                    method = "prodCode/structuralReplace",
                    duration_ms = format!("{:.2}ms", ms),
                    files = outcome.files.len(),
                    edits = outcome.total_edits(),
                    moves = outcome.moves.len(),
                    in_flight = remaining,
                    "✅ [LSP DONE]"
                );
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": workspace_edit_json(&outcome) })
            }
            Ok(Err(refused)) => {
                tracing::info!(req = req_num, session = session_id, method = "prodCode/structuralReplace", reason = %refused, "🚫 [LSP REFUSED]");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32602, "message": refused } })
            }
            Err(e) => {
                tracing::warn!(req = req_num, session = session_id, error = %e, "codemod failed");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": e.to_string() } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

fn lsp_rename(
    out_tx: &rapidfire::Sender<WireMessage>,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let uri = params
        .get("textDocument")
        .and_then(|td| td.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    let line = params
        .get("position")
        .and_then(|p| p.get("line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0) as u32;
    let col = params
        .get("position")
        .and_then(|p| p.get("character"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;
    let new_name = params
        .get("newName")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let file_path = PathBuf::from(uri.trim_start_matches("file://"));

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/rename",
        file = %file_path.display(),
        pos = format!("{}:{}", line + 1, col + 1),
        new_name = %new_name,
        in_flight,
        "🚀 [LSP START]"
    );

    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;

    tokio::task::spawn(async move {
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if let Err(e) = engine.activate_session(session_id) {
                    tracing::warn!(error = %e, session = session_id, "session view activation failed");
                }
                engine.rename(&fp_clone, line + 1, col + 1, &new_name)
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("rename task failed: {e}")))
        };
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let resp = match outcome {
            Ok(Ok(outcome)) => {
                tracing::info!(
                    req = req_num,
                    session = session_id,
                    method = "textDocument/rename",
                    duration_ms = format!("{:.2}ms", ms),
                    files = outcome.files.len(),
                    edits = outcome.total_edits(),
                    moves = outcome.moves.len(),
                    in_flight = remaining,
                    "✅ [LSP DONE]"
                );
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": workspace_edit_json(&outcome) })
            }
            Ok(Err(refused)) => {
                tracing::info!(req = req_num, session = session_id, method = "textDocument/rename", reason = %refused, "🚫 [LSP REFUSED]");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32602, "message": refused } })
            }
            Err(e) => {
                tracing::warn!(req = req_num, session = session_id, error = %e, "rename failed");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": e.to_string() } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

fn lsp_assists(
    out_tx: &rapidfire::Sender<WireMessage>,
    translator: &PathTranslator,
    view: &SessionView,
    method: Option<&str>,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let apply = method == Some("prodCode/applyAssist");
    let uri = params
        .get("textDocument")
        .and_then(|td| td.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    let line = params
        .pointer("/range/start/line")
        .and_then(|l| l.as_u64())
        .unwrap_or(0) as u32;
    let col = params
        .pointer("/range/start/character")
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;
    let end = match (
        params.pointer("/range/end/line").and_then(|l| l.as_u64()),
        params
            .pointer("/range/end/character")
            .and_then(|c| c.as_u64()),
    ) {
        (Some(l), Some(c)) => Some((l as u32 + 1, c as u32 + 1)),
        _ => None,
    };
    let assist_id = params
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let subtype = params
        .get("subtype")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let file_path = PathBuf::from(uri.trim_start_matches("file://"));

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    let method_name = if apply {
        "prodCode/applyAssist"
    } else {
        "prodCode/assists"
    };
    tracing::info!(req = req_num, session = view.session_id, method = method_name, file = %file_path.display(), pos = format!("{}:{}", line + 1, col + 1), assist = %assist_id, in_flight, "🚀 [LSP START]");

    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;

    tokio::task::spawn(async move {
        let result = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if let Err(e) = engine.activate_session(session_id) {
                    tracing::warn!(error = %e, session = session_id, "session view activation failed");
                }
                if apply {
                    engine
                        .apply_assist(&fp_clone, line + 1, col + 1, end, &assist_id, subtype)
                        .map(|r| r.map(|outcome| workspace_edit_json(&outcome)))
                } else {
                    engine
                        .list_assists(&fp_clone, line + 1, col + 1, end)
                        .map(|list| Ok(serde_json::json!(list)))
                }
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("assist task failed: {e}")))
        };
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let resp = match result {
            Ok(Ok(value)) => {
                tracing::info!(
                    req = req_num,
                    session = session_id,
                    method = method_name,
                    duration_ms = format!("{:.2}ms", ms),
                    in_flight = remaining,
                    "✅ [LSP DONE]"
                );
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": value })
            }
            Ok(Err(refused)) => {
                tracing::info!(req = req_num, session = session_id, method = method_name, reason = %refused, "🚫 [LSP REFUSED]");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32602, "message": refused } })
            }
            Err(e) => {
                tracing::warn!(req = req_num, session = session_id, error = %e, "assist failed");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": e.to_string() } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

fn lsp_workspace_symbol(
    out_tx: &rapidfire::Sender<WireMessage>,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let query = params
        .get("query")
        .and_then(|q| q.as_str())
        .unwrap_or("")
        .to_string();
    let limit = params.get("limit").and_then(|l| l.as_u64()).unwrap_or(64) as usize;
    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(req = req_num, session = view.session_id, method = "workspace/symbol", query = %query, in_flight, "🚀 [LSP START]");

    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;

    tokio::task::spawn(async move {
        let syms = {
            let mut engine = engine_arc.lock_owned().await;
            let q = query.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(e) = engine.activate_session(session_id) {
                    tracing::warn!(error = %e, session = session_id, "session view activation failed");
                }
                engine.workspace_symbols(&q, limit).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, session = session_id, "query failed");
                    Vec::new()
                })
            })
            .await
            .unwrap_or_default()
        };
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        tracing::info!(
            req = req_num,
            session = session_id,
            method = "workspace/symbol",
            duration_ms = format!("{:.2}ms", ms),
            symbols = syms.len(),
            in_flight = remaining,
            "✅ [LSP DONE]"
        );

        let sym_list: Vec<_> = syms.into_iter().map(|s| {
            serde_json::json!({
                "name": s.name,
                "kind": lsp_symbol_kind(&s.kind),
                "location": {
                    "uri": format!("file://{}", s.path.display()),
                    "range": {
                        "start": { "line": s.line.saturating_sub(1), "character": s.col.saturating_sub(1) },
                        "end": { "line": s.end_line.max(s.line).saturating_sub(1), "character": 0 }
                    }
                },
                "containerName": s.container
            })
        }).collect();
        let resp = serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": sym_list });
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

fn lsp_document_symbol(
    out_tx: &rapidfire::Sender<WireMessage>,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let uri = params
        .get("textDocument")
        .and_then(|td| td.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let file_path = PathBuf::from(uri.trim_start_matches("file://"));

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();

    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/documentSymbol",
        file = %file_path.display(),
        in_flight,
        "🚀 [LSP START]"
    );

    let engine_arc = Arc::clone(engine_lock);

    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;

    tokio::task::spawn(async move {
        let syms = {
            // Hold the engine for the whole query: activating the session view and
            // running the query under one lock keeps other sessions' buffers out and
            // prevents a concurrent edit from cancelling this snapshot.
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                 if let Err(e) = engine.activate_session(session_id) {
                     tracing::warn!(error = %e, session = session_id, "session view activation failed");
                 }
                 engine.document_symbols(&fp_clone).unwrap_or_else(|e| {
                     tracing::warn!(error = %e, session = session_id, "query failed");
                     Vec::new()
                 })
             })
             .await
             .unwrap_or_default()
        };

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let count = syms.len();

        if ms > 200.0 {
            SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                req = req_num,
                session = session_id,
                method = "textDocument/documentSymbol",
                duration_ms = format!("{:.2}ms", ms),
                symbols = count,
                in_flight = remaining,
                "⚠️ [LSP SLOW >200ms]"
            );
        } else {
            tracing::info!(
                req = req_num,
                session = session_id,
                method = "textDocument/documentSymbol",
                duration_ms = format!("{:.2}ms", ms),
                symbols = count,
                in_flight = remaining,
                "✅ [LSP DONE]"
            );
        }

        let sym_list: Vec<_> = syms.into_iter().map(|s| {
             let kind_num = lsp_symbol_kind(&s.kind);
             serde_json::json!({
                 "name": s.name,
                 "kind": kind_num,
                 "location": {
                     "uri": uri,
                     "range": {
                         "start": { "line": s.line.saturating_sub(1), "character": s.col.saturating_sub(1) },
                         "end": { "line": s.end_line.max(s.line).saturating_sub(1), "character": 0 }
                     }
                 },
                 "containerName": if s.containers.is_empty() { s.detail.clone() } else { Some(s.containers.join(" > ")) }
             })
         }).collect();

        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": sym_list
        });
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

fn lsp_references(
    out_tx: &rapidfire::Sender<WireMessage>,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let uri = params
        .get("textDocument")
        .and_then(|td| td.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    let line = params
        .get("position")
        .and_then(|p| p.get("line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0) as u32;
    let col = params
        .get("position")
        .and_then(|p| p.get("character"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;
    let file_path = PathBuf::from(uri.trim_start_matches("file://"));

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();

    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/references",
        file = %file_path.display(),
        pos = format!("{}:{}", line + 1, col + 1),
        in_flight,
        "🚀 [LSP START]"
    );

    let engine_arc = Arc::clone(engine_lock);

    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;

    tokio::task::spawn(async move {
        let refs = {
            // Hold the engine for the whole query: activating the session view and
            // running the query under one lock keeps other sessions' buffers out and
            // prevents a concurrent edit from cancelling this snapshot.
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if let Err(e) = engine.activate_session(session_id) {
                    tracing::warn!(error = %e, session = session_id, "session view activation failed");
                }
                engine.find_all_refs(&fp_clone, line + 1, col + 1).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, session = session_id, "query failed");
                    Vec::new()
                })
            })
            .await
            .unwrap_or_default()
        };

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let count = refs.len();

        if ms > 200.0 {
            SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                req = req_num,
                session = session_id,
                method = "textDocument/references",
                duration_ms = format!("{:.2}ms", ms),
                references = count,
                in_flight = remaining,
                "⚠️ [LSP SLOW >200ms]"
            );
        } else {
            tracing::info!(
                req = req_num,
                session = session_id,
                method = "textDocument/references",
                duration_ms = format!("{:.2}ms", ms),
                references = count,
                in_flight = remaining,
                "✅ [LSP DONE]"
            );
        }

        let locations: Vec<_> = refs.into_iter().map(|t| {
            serde_json::json!({
                "uri": format!("file://{}", t.path.display()),
                "range": {
                    "start": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) },
                    "end": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) }
                }
            })
        }).collect();

        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": locations
        });
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

fn lsp_definition(
    out_tx: &rapidfire::Sender<WireMessage>,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let uri = params
        .get("textDocument")
        .and_then(|td| td.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    let line = params
        .get("position")
        .and_then(|p| p.get("line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0) as u32;
    let col = params
        .get("position")
        .and_then(|p| p.get("character"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;
    let file_path = PathBuf::from(uri.trim_start_matches("file://"));

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();

    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/definition",
        file = %file_path.display(),
        pos = format!("{}:{}", line + 1, col + 1),
        in_flight,
        "🚀 [LSP START]"
    );

    let engine_arc = Arc::clone(engine_lock);

    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;

    tokio::task::spawn(async move {
        let defs = {
            // Hold the engine for the whole query: activating the session view and
            // running the query under one lock keeps other sessions' buffers out and
            // prevents a concurrent edit from cancelling this snapshot.
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if let Err(e) = engine.activate_session(session_id) {
                    tracing::warn!(error = %e, session = session_id, "session view activation failed");
                }
                engine.goto_definition(&fp_clone, line + 1, col + 1).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, session = session_id, "query failed");
                    Vec::new()
                })
            })
            .await
            .unwrap_or_default()
        };

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let count = defs.len();

        if ms > 200.0 {
            SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                req = req_num,
                session = session_id,
                method = "textDocument/definition",
                duration_ms = format!("{:.2}ms", ms),
                targets = count,
                in_flight = remaining,
                "⚠️ [LSP SLOW >200ms]"
            );
        } else {
            tracing::info!(
                req = req_num,
                session = session_id,
                method = "textDocument/definition",
                duration_ms = format!("{:.2}ms", ms),
                targets = count,
                in_flight = remaining,
                "✅ [LSP DONE]"
            );
        }

        let locations: Vec<_> = defs.into_iter().map(|t| {
            serde_json::json!({
                "uri": format!("file://{}", t.path.display()),
                "range": {
                    "start": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) },
                    "end": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) }
                }
            })
        }).collect();

        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": locations
        });
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

fn lsp_hover(
    out_tx: &rapidfire::Sender<WireMessage>,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let uri = params
        .get("textDocument")
        .and_then(|td| td.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    let line = params
        .get("position")
        .and_then(|p| p.get("line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0) as u32;
    let col = params
        .get("position")
        .and_then(|p| p.get("character"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0) as u32;
    let file_path = PathBuf::from(uri.trim_start_matches("file://"));

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();

    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/hover",
        file = %file_path.display(),
        pos = format!("{}:{}", line + 1, col + 1),
        in_flight,
        "🚀 [LSP START]"
    );

    // Acquire cheap snapshot (<1 µs) without holding mutex during query
    let engine_arc = Arc::clone(engine_lock);

    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;

    tokio::task::spawn(async move {
        let hover_res = {
            // Hold the engine for the whole query: activating the session view and
            // running the query under one lock keeps other sessions' buffers out and
            // prevents a concurrent edit from cancelling this snapshot.
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if let Err(e) = engine.activate_session(session_id) {
                    tracing::warn!(error = %e, session = session_id, "session view activation failed");
                }
                engine.hover(&fp_clone, line + 1, col + 1).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, session = session_id, "query failed");
                    None
                })
            })
            .await
            .unwrap_or(None)
        };

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let found = hover_res.is_some();

        if ms > 200.0 {
            SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                req = req_num,
                session = session_id,
                method = "textDocument/hover",
                duration_ms = format!("{:.2}ms", ms),
                found,
                in_flight = remaining,
                "⚠️ [LSP SLOW >200ms]"
            );
        } else {
            tracing::info!(
                req = req_num,
                session = session_id,
                method = "textDocument/hover",
                duration_ms = format!("{:.2}ms", ms),
                found,
                in_flight = remaining,
                "✅ [LSP DONE]"
            );
        }

        let resp = match hover_res {
            Some(markup) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "contents": {
                        "kind": "markdown",
                        "value": markup
                    }
                }
            }),
            None => serde_json::json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": null
            }),
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

/// Puts the user's toolchain directories (`~/.cargo/bin`, `~/go/bin`) first on PATH so remote
/// commands, rust-analyzer's `cargo metadata` and the gopls engine use the toolchains the
/// workspaces were built with, not a distro/snap binary a systemd user session resolves first.
/// The engines this host can actually serve: Rust is in-process, the others need their
/// language server on PATH. Clients place a workspace only on a node that lists its engine.
/// How long a probe for installed engines is reused. Engines appear when somebody installs
/// one, which is rare; the probe costs a `npm root -g` and half a dozen `which` calls, which
/// is 200 ms and more, and it used to run on every status, gossip and placement request.
const ENGINE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// The probe's answer and the moment it stops being used. An expiry rather than the time of
/// the probe, so that a test can seed an entry that is already stale without subtracting from
/// a monotonic clock.
type EngineCache = std::sync::RwLock<Option<(Instant, Vec<String>)>>;

fn engine_cache() -> &'static EngineCache {
    static CACHE: std::sync::OnceLock<EngineCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::RwLock::new(None))
}

/// [`available_engines`], answered from the last probe until it expires. Everything that
/// serves a request asks through here; the probe itself runs at startup and on the janitor's
/// tick, off the request path.
pub fn cached_available_engines() -> Vec<String> {
    if let Ok(cache) = engine_cache().read()
        && let Some((expires_at, engines)) = cache.as_ref()
        && Instant::now() < *expires_at
    {
        return engines.clone();
    }
    refresh_available_engines()
}

/// Probes for installed engines and stores the answer. Blocking: call it from a place that
/// is allowed to block, never from a request handler.
pub fn refresh_available_engines() -> Vec<String> {
    let engines = available_engines();
    store_engines(Instant::now() + ENGINE_CACHE_TTL, engines.clone());
    engines
}

fn store_engines(expires_at: Instant, engines: Vec<String>) {
    if let Ok(mut cache) = engine_cache().write() {
        *cache = Some((expires_at, engines));
    }
}

fn available_engines() -> Vec<String> {
    let mut engines = vec!["rust (ra_ap_ide)".to_string()];
    // gopls is useless without the go tool it drives ("no views" for every file).
    if prod_code_engine_generic::which_bin("gopls").is_ok()
        && prod_code_engine_generic::which_bin("go").is_ok()
    {
        engines.push("go (gopls)".to_string());
    }
    for engine in ["cpp", "swift", "python", "typescript"] {
        if let Some(server) = prod_code_engine_generic::GenericLspConfig::installed_server(engine) {
            engines.push(format!("{engine} ({server})"));
        }
    }
    engines.push("generic-lsp".to_string());
    engines
}

pub fn prefer_rustup_toolchain() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let home = PathBuf::from(home);
    let preferred: Vec<PathBuf> = [
        ".cargo/bin",
        "go/bin",
        ".local/go/bin",
        ".local/bin",
        ".npm-global/bin",
        ".bun/bin",
    ]
    .iter()
    .map(|rel| home.join(rel))
    .filter(|dir| dir.is_dir())
    .collect();
    if preferred.is_empty() {
        return;
    }
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<PathBuf> = std::env::split_paths(&current)
        .filter(|p| !preferred.contains(p))
        .collect();
    for dir in preferred.iter().rev() {
        paths.insert(0, dir.clone());
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        // SAFETY: called once at startup before any other thread exists.
        unsafe { std::env::set_var("PATH", joined) };
        tracing::info!(dirs = ?preferred, "user toolchain directories put first on PATH");
    }
}

/// Periodically unloads idle engines and prunes stale worktree workspace directories.
/// The address this gateway advertises: the bind address when it names a host, otherwise
/// the IPv4 of the interface that routes to the first peer (or to a public address) plus
/// the bind port.
fn detect_advertise_addr(bind: SocketAddr, first_peer: Option<&str>) -> String {
    if !bind.ip().is_unspecified() {
        return bind.to_string();
    }
    let probe = first_peer
        .and_then(|p| p.parse::<SocketAddr>().ok())
        .unwrap_or_else(|| "8.8.8.8:80".parse().unwrap());
    let local_ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect(probe).and_then(|_| s.local_addr()))
        .map(|a| a.ip())
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    format!("{local_ip}:{}", bind.port())
}

/// Whether a status lists `engine` among its engines (entries look like `swift (sourcekit-lsp)`).
fn cluster_supports_engine(status: &StatusResponse, engine: &str) -> bool {
    status.detected_engines.iter().any(|e| {
        e == engine
            || e.strip_prefix(engine)
                .is_some_and(|rest| rest.starts_with(' '))
    })
}

/// Sends this node's heartbeat to every known peer every [`GOSSIP_PERIOD`] and absorbs the
/// heartbeats they answer with, so every node ends up with the same picture of the cluster.
async fn gossip_loop(state: Arc<ServerState>) {
    loop {
        tokio::time::sleep(GOSSIP_PERIOD).await;
        let peers: Vec<String> = state.peers.read().await.iter().cloned().collect();
        if peers.is_empty() {
            continue;
        }
        let own = state.own_gossip().await;
        for peer in peers {
            let own = own.clone();
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                let reply = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    let addr: SocketAddr = peer.parse().ok()?;
                    let stream = TcpStream::connect(addr).await.ok()?;
                    let _ = stream.set_nodelay(true);
                    let mut framed = Framed::new(stream, ProdCodeCodec::new());
                    framed.send(WireMessage::Gossip(own)).await.ok()?;
                    match framed.next().await {
                        Some(Ok(WireMessage::Gossip(g))) => Some(g),
                        _ => None,
                    }
                })
                .await
                .ok()
                .flatten();
                if let Some(gossip) = reply {
                    state.absorb_gossip(gossip).await;
                }
            });
        }
    }
}

async fn janitor(state: Arc<ServerState>, idle_evict_secs: u64, prune_worktree_days: u64) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
    ticker.tick().await;
    loop {
        ticker.tick().await;
        // An engine installed while the daemon runs is picked up here, on a thread that is
        // allowed to block, instead of by the next request that needs the list.
        let _ = tokio::task::spawn_blocking(refresh_available_engines).await;
        if idle_evict_secs > 0 {
            let evicted = state
                .workspace_manager
                .evict_idle(std::time::Duration::from_secs(idle_evict_secs))
                .await;
            for root in evicted {
                tracing::info!(workspace = %root.display(), idle_secs = idle_evict_secs, "💤 [EVICT] unloaded idle workspace engine");
            }
        }
        if prune_worktree_days > 0 {
            workspace::prune_stale_worktree_dirs(
                &state.storage_root,
                std::time::Duration::from_secs(prune_worktree_days * 86_400),
                &state.workspace_manager,
            )
            .await;
        }
    }
}

/// Runs the gateway: bind, serve, and return when a signal says to stop.
pub async fn run(cli: ServerCli) -> Result<()> {
    prefer_rustup_toolchain();
    // Probe once here, while nothing is waiting on us, rather than on the first request.
    let engines = refresh_available_engines();
    tracing::info!(?engines, "engines detected");

    tracing::info!(
        "prod-code gateway daemon starting on {} (storage: {:?})",
        cli.bind,
        cli.storage
    );

    let mut state = ServerState::new(cli.storage);
    state.engine_allowlist = cli
        .engines
        .iter()
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    if !state.engine_allowlist.is_empty() {
        tracing::info!(engines = ?state.engine_allowlist, "serving only the listed engines");
    }
    if let Some(dir) = cli.shadow_dir.clone() {
        state.shadow_root = dir;
    }
    let swept = shadow::sweep(&state.shadow_root);
    if swept > 0 {
        tracing::info!(dir = %state.shadow_root.display(), swept, "removed leftover shadow directories");
    }
    match shadow::overlay_unavailable() {
        None => {
            tracing::info!(dir = %state.shadow_root.display(), "shadow runs: overlay mode (user namespaces + overlayfs)")
        }
        Some(reason) => tracing::info!(reason, "shadow runs: in-place mode"),
    }
    let state = Arc::new(state);
    let listener = TcpListener::bind(cli.bind).await?;
    // The address it actually bound, not the one it was asked for: with a port of 0, or an
    // interface that resolves to something else, those differ and only this one is reachable.
    let bound = listener.local_addr().unwrap_or(cli.bind);
    tracing::info!("prod-code gateway listening on {bound}");

    let peers: Vec<String> = cli
        .peers
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    let advertise = cli
        .advertise
        .clone()
        .unwrap_or_else(|| detect_advertise_addr(cli.bind, peers.first().map(String::as_str)));
    *state.advertise.write().await = advertise.clone();
    {
        let mut set = state.peers.write().await;
        for p in &peers {
            if *p != advertise {
                set.insert(p.clone());
            }
        }
    }
    tracing::info!(advertise, peers = ?peers, "cluster identity");
    tokio::spawn(gossip_loop(Arc::clone(&state)));
    if let Some(rx) = state.metrics.take_receiver() {
        tokio::spawn(metrics::run_writer(state.metrics.dir().to_path_buf(), rx));
    }

    tokio::spawn(janitor(
        Arc::clone(&state),
        cli.idle_evict_secs,
        cli.prune_worktree_days,
    ));

    // A gateway is stopped by its supervisor (launchd, systemd) and by a deploy script, both
    // of which send SIGTERM and then wait. Without a handler the process dies where it stands:
    // in-flight queries are cut, and nothing that runs at exit runs. Stopping the accept loop
    // and returning normally is all that is needed — every session is a task holding its own
    // socket, and the client treats a closed connection as a session to re-open.
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    loop {
        let (socket, addr) = tokio::select! {
            accepted = listener.accept() => accepted?,
            _ = terminate.recv() => {
                tracing::info!("SIGTERM: no longer accepting connections");
                return Ok(());
            }
            _ = interrupt.recv() => {
                tracing::info!("SIGINT: no longer accepting connections");
                return Ok(());
            }
        };
        // Small request/response frames must not wait for delayed ACKs (Nagle).
        let _ = socket.set_nodelay(true);
        let state_clone = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(err) = handle_client(socket, addr, state_clone).await {
                tracing::error!(%addr, %err, "Error in client connection");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory renamed or deleted locally leaves nothing behind on the copy (#124): the files
    /// the manifest no longer lists go, and so do the directories that held only them, while a
    /// directory that still holds a file, the workspace root and the per-node caches stay.
    #[test]
    fn a_directory_gone_locally_is_gone_from_the_copy() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for (rel, body) in [
            ("Sources/OldApp/main.swift", "old"),
            ("Sources/Unused/deep/x.swift", "unused"),
            ("Sources/NewApp/main.swift", "new"),
            ("Package.swift", "pkg"),
            (".build/debug/cache.o", "cache"),
        ] {
            std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
            std::fs::write(root.join(rel), body).unwrap();
        }
        std::fs::create_dir_all(root.join("Sources/Empty/deeper")).unwrap();
        let stamp = |rel: &str, body: &str| FileStamp {
            relative_path: rel.to_string(),
            size: body.len() as u64,
            hash: content_hash(body.as_bytes()),
        };
        let manifest = [
            stamp("Sources/NewApp/main.swift", "new"),
            stamp("Package.swift", "pkg"),
        ];
        let (missing, mut deleted) = reconcile_manifest(root, &manifest);
        deleted.sort();
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(
            deleted,
            ["Sources/OldApp/main.swift", "Sources/Unused/deep/x.swift"]
        );
        for gone in ["Sources/OldApp", "Sources/Unused", "Sources/Empty"] {
            assert!(!root.join(gone).exists(), "{gone} is still on the copy");
        }
        assert!(root.join("Sources/NewApp/main.swift").is_file());
        assert!(
            root.join(".build/debug/cache.o").is_file(),
            "a node cache is not touched"
        );

        // A single deletion climbs only as far as the directories it empties.
        std::fs::create_dir_all(root.join("a/b/c")).unwrap();
        std::fs::write(root.join("a/keep.rs"), "k").unwrap();
        std::fs::write(root.join("a/b/c/gone.rs"), "g").unwrap();
        std::fs::remove_file(root.join("a/b/c/gone.rs")).unwrap();
        let emptied = root.join("a/b/c");
        prune_empty_parents(root, Some(&emptied));
        assert!(!root.join("a/b").exists());
        assert!(root.join("a/keep.rs").is_file());
        prune_empty_parents(root, Some(root));
        assert!(root.exists(), "the workspace root itself is never removed");
    }

    #[test]
    fn test_changed_since_reports_new_changed_and_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("src/a.rs"), "a").unwrap();
        std::fs::write(root.join("src/gone.rs"), "g").unwrap();
        std::fs::write(root.join("Cargo.lock"), "l1").unwrap();
        let before = snapshot_tree(root);

        std::fs::write(root.join("src/a.rs"), "a formatted").unwrap();
        std::fs::write(root.join("src/new.rs"), "n").unwrap();
        std::fs::remove_file(root.join("src/gone.rs")).unwrap();
        std::fs::write(root.join("target/debug/junk.o"), "x").unwrap();

        let changed = changed_since(root, &before);
        let names: Vec<(&str, bool)> = changed
            .iter()
            .map(|f| (f.relative_path.as_str(), f.content.is_some()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("src/a.rs", true),
                ("src/gone.rs", false),
                ("src/new.rs", true)
            ]
        );
        assert_eq!(
            changed[0].content.as_deref(),
            Some(b"a formatted".as_slice())
        );
    }

    #[tokio::test]
    async fn test_delta_sync_reports_fresh_until_probed() {
        let storage = tempfile::tempdir().unwrap();
        let manager = WorkspaceManager::new();
        let delta = || SyncRequest {
            client_workspace_root: "/tmp/ws".to_string(),
            files: vec![FileDelta {
                relative_path: "src/lib.rs".to_string(),
                content: Some(b"fn a() {}".to_vec()),
                is_executable: false,
            }],
            clean_others: false,
            base_workspace_name: Some("ws".to_string()),
        };
        let first = apply_sync(storage.path(), &manager, delta()).await;
        assert!(
            first.workspace_was_fresh,
            "nobody established this workspace yet"
        );
        let probe = apply_sync_probe(
            storage.path(),
            &manager,
            SyncProbeRequest {
                client_workspace_root: "/tmp/ws".to_string(),
                base_workspace_name: Some("ws".to_string()),
                seed_from: None,
                files: vec![FileStamp {
                    relative_path: "src/lib.rs".to_string(),
                    size: 9,
                    hash: content_hash(b"fn a() {}"),
                }],
            },
        )
        .await;
        assert!(probe.missing.is_empty());
        let second = apply_sync(storage.path(), &manager, delta()).await;
        assert!(
            !second.workspace_was_fresh,
            "probe established the workspace"
        );
    }

    #[tokio::test]
    async fn test_sync_probe_seeds_and_reconciles() {
        let storage = tempfile::tempdir().unwrap();
        let origin = storage.path().join("repo");
        std::fs::create_dir_all(origin.join("src")).unwrap();
        std::fs::write(origin.join("Cargo.toml"), "[package]\nname = \"repo\"\n").unwrap();
        std::fs::write(origin.join("src/lib.rs"), "pub fn a() {}").unwrap();
        std::fs::write(origin.join("src/only_in_origin.rs"), "pub fn gone() {}").unwrap();
        let manager = WorkspaceManager::new();

        let req = SyncProbeRequest {
            client_workspace_root: "/tmp/wt".to_string(),
            base_workspace_name: Some("repo--wt-0001".to_string()),
            seed_from: Some("repo".to_string()),
            files: vec![
                FileStamp {
                    relative_path: "Cargo.toml".to_string(),
                    size: 24,
                    hash: content_hash(b"[package]\nname = \"repo\"\n"),
                },
                FileStamp {
                    relative_path: "src/lib.rs".to_string(),
                    size: 21,
                    hash: content_hash(b"pub fn a() -> u8 {}"),
                },
                FileStamp {
                    relative_path: "src/new.rs".to_string(),
                    size: 3,
                    hash: content_hash(b"// n"),
                },
            ],
        };
        let resp = apply_sync_probe(storage.path(), &manager, req).await;
        assert!(resp.seeded);
        assert_eq!(resp.files_deleted, 1);
        assert_eq!(
            resp.missing,
            vec!["src/lib.rs".to_string(), "src/new.rs".to_string()]
        );
        let wt = storage.path().join("repo--wt-0001");
        assert!(wt.join("Cargo.toml").exists());
        assert!(!wt.join("src/only_in_origin.rs").exists());
        assert!(
            origin.join("src/only_in_origin.rs").exists(),
            "origin copy untouched"
        );

        // A second probe on the now-populated directory does not seed again.
        let again = apply_sync_probe(
            storage.path(),
            &manager,
            SyncProbeRequest {
                client_workspace_root: "/tmp/wt".to_string(),
                base_workspace_name: Some("repo--wt-0001".to_string()),
                seed_from: Some("repo".to_string()),
                files: vec![FileStamp {
                    relative_path: "Cargo.toml".to_string(),
                    size: 24,
                    hash: content_hash(b"[package]\nname = \"repo\"\n"),
                }],
            },
        )
        .await;
        assert!(!again.seeded);
        assert!(again.missing.is_empty());
        assert_eq!(
            again.files_deleted, 1,
            "src/lib.rs is not in the manifest any more"
        );
    }

    #[tokio::test]
    async fn test_server_state_status() {
        let temp = tempfile::tempdir().unwrap();
        let state = ServerState::new(temp.path().to_path_buf());
        let status = state.status().await;
        assert_eq!(status.server_pid, std::process::id());
        assert_eq!(status.active_sessions, 0);
        assert_eq!(status.loaded_workspaces, 0);
        assert!(
            status
                .detected_engines
                .contains(&"rust (ra_ap_ide)".to_string())
        );
        assert!(state.serves_engine("rust") && state.serves_engine("swift"));
    }

    #[tokio::test]
    async fn test_engine_allowlist_narrows_advertised_engines() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = ServerState::new(temp.path().to_path_buf());
        state.engine_allowlist = vec!["swift".to_string()];
        let status = state.status().await;
        assert!(
            !status
                .detected_engines
                .iter()
                .any(|e| e.starts_with("rust") || e == "generic-lsp"),
            "{:?}",
            status.detected_engines
        );
        assert!(
            status
                .detected_engines
                .iter()
                .all(|e| e.starts_with("swift"))
        );
        assert!(state.serves_engine("swift") && state.serves_engine("Swift"));
        assert!(!state.serves_engine("rust") && !state.serves_engine("generic"));
    }

    #[test]
    fn test_engine_detection() {
        use prod_code_protocol::messages::EngineKind;

        let temp = tempfile::tempdir().unwrap();
        assert_eq!(detect_engine(temp.path()), EngineKind::Generic);

        std::fs::write(temp.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_engine(temp.path()), EngineKind::Rust);

        let go_temp = tempfile::tempdir().unwrap();
        std::fs::write(go_temp.path().join("go.mod"), "").unwrap();
        assert_eq!(detect_engine(go_temp.path()), EngineKind::Go);

        let py_temp = tempfile::tempdir().unwrap();
        std::fs::write(py_temp.path().join("pyproject.toml"), "").unwrap();
        assert_eq!(detect_engine(py_temp.path()), EngineKind::Python);

        let ts_temp = tempfile::tempdir().unwrap();
        std::fs::write(ts_temp.path().join("package.json"), "").unwrap();
        assert_eq!(detect_engine(ts_temp.path()), EngineKind::TypeScript);
    }

    #[tokio::test]
    async fn test_apply_sync_create_and_delete() {
        use prod_code_protocol::FileDelta;

        let storage_temp = tempfile::tempdir().unwrap();
        let client_root = "/Users/testuser/Projects/my-app";

        let req = SyncRequest {
            client_workspace_root: client_root.to_string(),
            files: vec![
                FileDelta {
                    relative_path: "src/lib.rs".to_string(),
                    content: Some(b"pub fn add(a: i32, b: i32) -> i32 { a + b }".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: "README.md".to_string(),
                    content: Some(b"# My App".to_vec()),
                    is_executable: false,
                },
            ],
            clean_others: false,
            base_workspace_name: None,
        };

        let resp = apply_sync(storage_temp.path(), &WorkspaceManager::new(), req).await;
        assert_eq!(resp.files_updated, 2);
        assert_eq!(resp.files_deleted, 0);

        let app_dir = storage_temp.path().join("my-app");
        assert!(app_dir.join("src/lib.rs").exists());
        assert!(app_dir.join("README.md").exists());
        let content = std::fs::read_to_string(app_dir.join("src/lib.rs")).unwrap();
        assert!(content.contains("pub fn add"));

        // Now test deleting README.md
        let del_req = SyncRequest {
            client_workspace_root: client_root.to_string(),
            files: vec![FileDelta {
                relative_path: "README.md".to_string(),
                content: None,
                is_executable: false,
            }],
            clean_others: false,
            base_workspace_name: None,
        };

        let del_resp = apply_sync(storage_temp.path(), &WorkspaceManager::new(), del_req).await;
        assert_eq!(del_resp.files_updated, 0);
        assert_eq!(del_resp.files_deleted, 1);
        assert!(!app_dir.join("README.md").exists());
        assert!(app_dir.join("src/lib.rs").exists());
    }

    /// Both halves of the engine cache, in one test because the cache is process-global and
    /// two tests would race for it. The sentinel is a value the probe cannot produce, so a
    /// sentinel coming back proves the probe did not run, and a sentinel gone proves it did.
    #[test]
    fn engines_are_served_from_the_cache_until_it_expires() {
        let sentinel = vec!["sentinel (not a real engine)".to_string()];

        store_engines(Instant::now() + ENGINE_CACHE_TTL, sentinel.clone());
        assert_eq!(
            cached_available_engines(),
            sentinel,
            "a live entry must be answered without probing"
        );

        store_engines(Instant::now(), sentinel.clone());
        let fresh = cached_available_engines();
        assert_ne!(fresh, sentinel, "an expired entry must be probed again");
        assert!(
            fresh.iter().any(|e| e.starts_with("rust ")),
            "the probe always reports the in-process Rust engine, got {fresh:?}"
        );
        assert_eq!(
            cached_available_engines(),
            fresh,
            "the probe's answer is what the next caller gets"
        );
    }
}

#[cfg(test)]
mod analyzer_panic_tests {
    use super::*;

    #[test]
    fn a_panic_is_reported_as_one_unchecked_file_not_as_a_failed_request() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("escaping bound vars.".to_string());
        let message = panic_message(payload);
        assert_eq!(message, "escaping bound vars.");
        assert_eq!(panic_message(Box::new("static text")), "static text");
        assert_eq!(panic_message(Box::new(42u8)), "no message");

        let report = analyzer_panic_report(&message);
        let items = report["items"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["severity"], 1);
        assert_eq!(items[0]["code"], ANALYZER_PANIC);
        assert_eq!(items[0]["range"]["start"]["line"], 0);
        let text = items[0]["message"].as_str().unwrap();
        assert!(text.contains("nothing in it was checked"), "{text}");
        assert!(
            text.contains("escaping bound vars. The compiler"),
            "one full stop: {text}"
        );
        assert!(text.contains("verify"), "{text}");
    }
}
