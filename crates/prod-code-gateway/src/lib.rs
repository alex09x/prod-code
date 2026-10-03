//! prod-code gateway daemon: multi-tenant server for remote code intelligence over 10 GbE LAN.
//!
//! The daemon is a library with a thin binary on top, so that the pieces a socket normally
//! stands in front of — the workspace manager, the dispatch, the language server backends —
//! can be exercised directly by tests.

pub mod admission;
pub mod backend;
pub mod cpp_index;
pub mod detect;
pub mod editor_proxy;
pub mod embed;
pub mod exec_shim;
pub mod memory;
mod metrics;
pub mod priming;
pub mod python_cache;
pub mod search;
pub mod shadow;
pub mod swift_cache;
pub mod ts_cache;
pub mod workspace;

pub use detect::detect_engine;

use anyhow::Result;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    AnyStream, ClusterResponse, ExecChanges, ExecChunk, ExecExit, ExecRequest, FileDelta, FileStamp,
    HandshakeResponse, LoadedWorkspaceInfo, NodeGossip, PathTranslator, PeerInfo, PlaceRequest,
    PlaceResponse, ProdCodeCodec, RemoteExecCommand, RemoteExecFormat, RemoteExecLanguage, RemoteExecRequest,
    RemoteExecResult, RemoteExecStream, RemoteExecTestEvent, StatusResponse, SyncProbeRequest,
    SyncProbeResponse, SyncRequest, SyncResponse, WireMessage, content_hash,
    negotiate_protocol_version, parse_cargo_json_event, parse_go_test_json_event,
    path::{file_uri, uri_or_path}, ScrubSecrets,
};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;
use workspace::{SessionView, WorkspaceManager};

pub static NEXT_REQ_ID: AtomicU64 = AtomicU64::new(1);
pub static ACTIVE_QUERIES: AtomicUsize = AtomicUsize::new(0);

/// Remote commands running now, by a per-process id: workspace directory name, command line and
/// start. Status answers list them, so that a node running a build or test is not taken for
/// idle and restarted under it (#273).
type RunningTable = std::collections::HashMap<u64, (String, String, Instant)>;
static RUNNING_COMMANDS: std::sync::LazyLock<std::sync::Mutex<RunningTable>> =
    std::sync::LazyLock::new(Default::default);
static NEXT_COMMAND_ID: AtomicU64 = AtomicU64::new(0);

/// A command's entry in [`RUNNING_COMMANDS`], removed when the command's handler returns,
/// however it returns.
struct RunningEntry(u64);

impl RunningEntry {
    fn start(workspace: &std::path::Path, command: &[String]) -> Self {
        let id = NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed);
        let name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        RUNNING_COMMANDS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, (name, command.join(" "), Instant::now()));
        Self(id)
    }
}

impl Drop for RunningEntry {
    fn drop(&mut self) {
        RUNNING_COMMANDS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// The commands running now, as a status answer reports them.
fn running_commands() -> Vec<prod_code_protocol::RunningCommand> {
    RUNNING_COMMANDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .map(
            |(workspace, command, started)| prod_code_protocol::RunningCommand {
                workspace: workspace.clone(),
                command: command.clone(),
                running_seconds: started.elapsed().as_secs(),
            },
        )
        .collect()
}
pub static TOTAL_QUERIES: AtomicU64 = AtomicU64::new(0);
pub static SLOW_QUERIES: AtomicU64 = AtomicU64::new(0);

/// Ensure standard I/O file descriptors are in blocking mode (#806, #807).
///
/// When the gateway daemon runs under systemd with journald or under piped supervision,
/// some supervisors or previous subprocesses may leave stdin, stdout, or stderr with O_NONBLOCK set.
/// If tracing or rust-analyzer writes to a non-blocking stderr while the socket buffer is full,
/// standard library `eprintln!` panics with EAGAIN ("os error 11: Resource temporarily unavailable").
#[cfg(unix)]
pub fn ensure_blocking_stdio() {
    for fd in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 && (flags & libc::O_NONBLOCK) != 0 {
                libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
            }
        }
    }
}

#[cfg(not(unix))]
pub fn ensure_blocking_stdio() {}

#[cfg(unix)]
fn get_hostname() -> String {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0
        && let Some(pos) = buf.iter().position(|&b| b == 0) {
            return String::from_utf8_lossy(&buf[..pos]).to_string();
        }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "node".to_string())
}

#[cfg(not(unix))]
fn get_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "node".to_string())
}

struct ActiveSession<'a>(&'a AtomicUsize);

impl<'a> ActiveSession<'a> {
    fn start(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl Drop for ActiveSession<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Parser, Debug, Clone)]
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

    /// Optional Unix domain socket path to bind for local transport.
    #[arg(long, env = "PROD_CODE_SOCKET")]
    pub socket_path: Option<PathBuf>,

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

    /// Memory a new engine is counted at until it has loaded and settled, in MiB, when deciding
    /// whether the host can take it without passing 85% in use (#433). 0: by engine, 4 GiB for
    /// Rust, 1 GiB for the other language servers.
    #[arg(long, env = "PROD_CODE_ENGINE_RESERVE_MIB", default_value_t = 0)]
    pub engine_reserve_mib: u64,

    /// Maximum concurrent cold engine loads permitted simultaneously (0: automatic based on CPU count).
    /// Bounding concurrent cold engine loads prevents CPU and thread contention when multiple
    /// worktrees initialize at once (#408).
    #[arg(
        long,
        env = "PROD_CODE_MAX_CONCURRENT_ENGINE_LOADS",
        default_value_t = 0
    )]
    pub max_concurrent_engine_loads: usize,

    /// Delete `<repo>--wt-*` workspace directories unused for this many seconds (0 disables). Defaults to 3600 (1 hour).
    #[arg(long, env = "PROD_CODE_PRUNE_WORKTREE_SECS", default_value_t = 3600)]
    pub prune_worktree_secs: u64,

    /// Delete `<repo>--wt-*` workspace directories unused for this many days (0 disables). Overrides `--prune-worktree-secs` if set.
    #[arg(long, env = "PROD_CODE_PRUNE_WORKTREE_DAYS")]
    pub prune_worktree_days: Option<u64>,

    /// Delete main (non-worktree) workspace directories unused for this many seconds (0 disables). Defaults to 86400 (24 hours).
    #[arg(long, env = "PROD_CODE_PRUNE_WORKSPACE_SECS", default_value_t = 86400)]
    pub prune_workspace_secs: u64,

    /// Delete main (non-worktree) workspace directories unused for this many days (0 disables). Overrides `--prune-workspace-secs` if set.
    #[arg(long, env = "PROD_CODE_PRUNE_WORKSPACE_DAYS")]
    pub prune_workspace_days: Option<u64>,

    /// Below this share of free space (percent) on the storage filesystem, idle worktree copies
    /// are deleted oldest first, however young, until it is reached again (0 disables) (#386).
    #[arg(long, env = "PROD_CODE_PRUNE_BELOW_FREE_PERCENT", default_value_t = 15)]
    pub prune_below_free_percent: u64,

    /// Only serve these engines (comma-separated: rust, go, cpp, swift, python, typescript).
    /// The node advertises nothing else, so placement never sends other work here, and a
    /// handshake for another engine is refused. Empty: every installed engine.
    #[arg(long, env = "PROD_CODE_ENGINES", value_delimiter = ',')]
    pub engines: Vec<String>,

    /// Directory for the overlays of shadow runs (one upper directory per hypothesis, holding
    /// what its build wrote). Default: a storage-specific directory next to the storage directory;
    /// a tmpfs path (`/dev/shm/prod-code-shadow`) keeps hypothesis builds in RAM.
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

    /// Enable RAM-disk build cache (/dev/shm or tmpfs) for fast compilation (Roadmap 6.2).
    #[arg(long, env = "PROD_CODE_BUILD_RAM", default_value_t = false)]
    pub build_cache_ram: bool,

    /// Directory for RAM-disk or fast build caches. Default: `/dev/shm/prod-code-build` on Linux,
    /// or a tmpfs directory.
    #[arg(long, env = "PROD_CODE_BUILD_CACHE_DIR")]
    pub build_cache_dir: Option<PathBuf>,
}

impl ServerCli {
    /// Effective timeout for pruning stale worktree directories, in seconds (0 disables).
    pub fn effective_prune_worktree_secs(&self) -> u64 {
        match self.prune_worktree_days {
            Some(days) => days.saturating_mul(86_400),
            None => self.prune_worktree_secs,
        }
    }

    /// Effective timeout for pruning stale main workspace directories, in seconds (0 disables).
    pub fn effective_prune_workspace_secs(&self) -> u64 {
        match self.prune_workspace_days {
            Some(days) => days.saturating_mul(86_400),
            None => self.prune_workspace_secs,
        }
    }
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
    /// Exclusive ownership follows every state clone held by an accepted session.
    ///
    /// Only gateway startup installs this guard; constructed test and embedding states do not
    /// claim a filesystem namespace until they explicitly opt into one.
    _shadow_root_owner: Option<shadow::ShadowRootOwner>,
    /// Per-workspace declaration indexes for `code_search`.
    pub search_indexes: search::SearchIndexes,
    /// The token every connection must open with (#402); `None` takes every connection.
    pub auth_token: Option<String>,
    /// Outbound client TLS credentials / configuration when TLS is enabled.
    pub client_tls: Option<prod_code_protocol::transport::BuiltClientTls>,
    /// Whether RAM-disk build caching in `/dev/shm` or tmpfs is enabled (Roadmap 6.2).
    pub build_cache_ram: bool,
    /// Custom path for RAM-disk or fast build caches (`--build-cache-dir`).
    pub build_cache_dir: Option<PathBuf>,
}

/// How long a connection to a gateway that requires a token may take to send it.
const AUTH_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// What a connection without the cluster's token is told before it is closed.
const AUTH_REFUSED: &str = "this gateway requires the cluster's connection token: set PROD_CODE_AUTH_TOKEN to it, or PROD_CODE_AUTH_TOKEN_FILE to a file that holds it";

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
    pub storage_root: PathBuf,
    pub metrics: Arc<metrics::Metrics>,
    /// The session is an editor's ([`prod_code_protocol::PURPOSE_EDITOR`]): the Rust engine
    /// pushes the diagnostics of every document it opens or changes.
    pub editor: bool,
    /// Per document, how many edits the session has sent: a diagnostics pass waits out a burst
    /// of typing and runs only for the last edit of it.
    pub edits: Arc<std::sync::Mutex<std::collections::HashMap<PathBuf, u64>>>,
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
/// How long a silent peer stays in the cluster before eviction.
const PEER_EVICT: std::time::Duration = std::time::Duration::from_secs(60);
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
            _shadow_root_owner: None,
            search_indexes: search::SearchIndexes::with_model_dir(embed::model_dir(&storage_root)),
            storage_root,
            workspace_manager: Arc::new(WorkspaceManager::new()),
            advertise: tokio::sync::RwLock::new(String::new()),
            peers: tokio::sync::RwLock::new(std::collections::BTreeSet::new()),
            cluster: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            metrics: Arc::new(metrics::Metrics::new(metrics_dir)),
            engine_allowlist: Vec::new(),
            auth_token: None,
            client_tls: None,
            build_cache_ram: false,
            build_cache_dir: None,
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
        let available = cached_available_engines();
        if self.engine_allowlist.is_empty() {
            return available;
        }
        let filtered: Vec<String> = available
            .into_iter()
            .filter(|entry| {
                let name = entry.split(' ').next().unwrap_or(entry.as_str());
                let name = name.strip_suffix("-lsp").unwrap_or(name);
                self.serves_engine(name)
            })
            .collect();
        if !filtered.is_empty() {
            filtered
        } else {
            self.engine_allowlist.clone()
        }
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
        let peers = {
            let cluster = self.cluster.read().await;
            self.peers
                .read()
                .await
                .iter()
                .filter(|p| {
                    cluster
                        .get(*p)
                        .map(|e| e.last_seen.elapsed() < PEER_ALIVE)
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        };
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
        let own_addr = self.advertise.read().await.clone();
        let resp = place_in(req, view);
        if req.rebalance_active
            && let Some(ref target) = resp.node
            && target != &own_addr
        {
            let notified = self
                .workspace_manager
                .trigger_rebalance_by_name(
                    &req.workspace_name,
                    target.clone(),
                    Some(resp.reason.clone()),
                )
                .await;
            if notified > 0 {
                tracing::info!(
                    workspace = %req.workspace_name,
                    target = %target,
                    notified,
                    "triggered dynamic rebalance redirect for active sessions"
                );
            }
        }
        resp
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
            platform: Some(prod_code_protocol::platform()),
            running_commands: running_commands(),
            host: self.host_resources(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            git_commit: Some(prod_code_protocol::git_commit().to_string())
                .filter(|c| c != "unknown"),
        }
    }

    /// What this host has left, less the memory held for engines still loading: placement on
    /// the other nodes then sees a load this one has admitted before the load shows (#433).
    fn host_resources(&self) -> prod_code_protocol::HostResources {
        let mut host = memory::host_resources(&self.storage_root);
        let reserved = self.workspace_manager.admission().reserved_bytes();
        host.memory_available_bytes = host
            .memory_available_bytes
            .map(|available| available.saturating_sub(reserved));
        host
    }
}

/// The node `req` should be placed on, given the cluster `view`: the one that already holds it,
/// otherwise the quietest live node that can serve it and is not short of memory or disk. A
/// holder short of either gives up an idle workspace the way an overloaded one does. A macOS
/// node takes only work that needs macOS, or that no other live node can serve (#308): it is a
/// developer's Mac, running Swift and Go with macOS-only cgo, and plain Go or Rust is placed on
/// the Linux nodes even when the Mac is quieter.
fn place_in(req: &PlaceRequest, view: ClusterResponse) -> PlaceResponse {
    let engine = req.engine.as_deref();
    // A node started with `--engines swift` advertises one engine and serves nothing
    // else. A workspace whose engine the client could not determine must not be sent
    // there: it would be refused at the handshake, or worse, accepted by an older
    // gateway that does not know it is specialised.
    // A node that reports no platform is an older gateway: it cannot be shown to run the
    // OS the workspace needs, so it does not get it.
    let runs_os = |n: &PeerInfo| {
        req.os.as_deref().is_none_or(|os| {
            n.status
                .platform
                .as_deref()
                .is_some_and(|p| p.starts_with(os))
        })
    };
    let capable = |n: &PeerInfo| {
        runs_os(n)
            && match engine {
                Some(e) => cluster_supports_engine(&n.status, e),
                None => n.status.detected_engines.len() > 1,
            }
    };
    let on_macos = |n: &PeerInfo| {
        n.status
            .platform
            .as_deref()
            .is_some_and(|p| p.starts_with("macos"))
    };
    let other_than_macos = req.os.is_none()
        && view
            .nodes
            .iter()
            .any(|n| n.alive && capable(n) && !on_macos(n));
    let capable = |n: &PeerInfo| capable(n) && !(other_than_macos && on_macos(n));
    // A node short of memory or disk takes no new workspace while a capable one that is not can
    // (#396): a full disk truncates the files synced to it (#385), and an engine loaded into a
    // host out of memory pushes it into swap.
    let pressure = |n: &PeerInfo| n.status.host.pressure();
    let roomy_exists = view
        .nodes
        .iter()
        .any(|n| n.alive && capable(n) && pressure(n).is_none());
    let takes_new = |n: &PeerInfo| !(roomy_exists && pressure(n).is_some());
    let score = |n: &PeerInfo| n.status.congestion_score();
    let quietest = view
        .nodes
        .iter()
        .filter(|n| n.alive && capable(n) && takes_new(n))
        .min_by(|a, b| {
            score(a)
                .partial_cmp(&score(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    let holder = view.nodes.iter().find(|n| {
        n.alive && capable(n) && n.workspaces.iter().any(|w| w.name == req.workspace_name)
    });
    if let Some(h) = holder {
        let (idle, sessions) = h
            .workspaces
            .iter()
            .find(|w| w.name == req.workspace_name)
            .map(|w| (w.sessions == 0, w.sessions))
            .unwrap_or((true, 0));
        let can_move = idle || req.rebalance_active;
        if let Some(q) = quietest
            && can_move
            && q.addr != h.addr
        {
            let state_str = if idle {
                "idle".to_string()
            } else {
                format!("active, {sessions} sessions")
            };
            if let Some(why) = pressure(h)
                && roomy_exists
            {
                return PlaceResponse {
                    node: Some(q.addr.clone()),
                    reason: format!(
                        "moved from {} ({why}, {state_str}) to {} (score {:.2}, load {:.2}/cpu)",
                        h.addr,
                        q.addr,
                        score(q),
                        q.status.load_per_cpu().unwrap_or(0.0)
                    ),
                };
            }
            if score(h) >= 0.80 && score(q) < score(h) * 0.50 && (score(h) - score(q)) >= 0.40 {
                return PlaceResponse {
                    node: Some(q.addr.clone()),
                    reason: format!(
                        "rebalanced from {} (score {:.2}, {state_str}) to the quieter {} (score {:.2})",
                        h.addr,
                        score(h),
                        q.addr,
                        score(q)
                    ),
                };
            }
        }
        return PlaceResponse {
            node: Some(h.addr.clone()),
            reason: format!("already loaded on {}", h.addr),
        };
    }
    match quietest {
        Some(q) => {
            let mut reason = format!(
                "quietest node serving {} (score {:.2}, load {:.2}/cpu)",
                engine.unwrap_or("any engine"),
                score(q),
                q.status.load_per_cpu().unwrap_or(0.0)
            );
            let passed_over: Vec<String> = view
                .nodes
                .iter()
                .filter(|n| n.alive && capable(n) && !takes_new(n))
                .filter_map(|n| pressure(n).map(|why| format!("{} ({why})", n.addr)))
                .collect();
            if !passed_over.is_empty() {
                reason.push_str(&format!("; passed over {}", passed_over.join(", ")));
            }
            if let Some(why) = pressure(q) {
                reason.push_str(&format!(
                    "; it is short too ({why}), as is every node serving it"
                ));
            }
            PlaceResponse {
                node: Some(q.addr.clone()),
                reason,
            }
        }
        None => PlaceResponse {
            node: None,
            reason: match req.os.as_deref() {
                Some(os) => format!(
                    "no live node runs {os} and serves {}",
                    engine.unwrap_or("this workspace")
                ),
                None => {
                    format!("no live node serves {}", engine.unwrap_or("this workspace"))
                }
            },
        },
    }
}

/// The parts of a cargo target directory that a new worktree's copy takes from the main copy
/// when it is seeded: compiled crates, build-script outputs and cargo's fingerprints. A registry
/// crate has the same source path in every copy, so its fingerprint still matches and it is not
/// compiled again; only the workspace's own crates are (#278: 8 crates in 37.0 s against 305 in
/// 106.2 s). Incremental caches belong to those crates and are left behind. The copy is the
/// worktree's own: nothing is shared afterwards, so no build waits on another's lock.
const SEEDED_BUILD_DIRS: &[&str] = &["deps", "build", ".fingerprint"];

/// Free and total bytes of a filesystem.
#[derive(Debug, Clone, Copy)]
pub struct DiskSpace {
    pub free: u64,
    pub total: u64,
}

/// The share of its filesystem a seed must leave free (#419): above the janitor's 15% prune
/// line (#386), so seeding a worktree never pushes the node into pruning or into the disk
/// pressure that placement avoids (#396).
const SEED_MIN_FREE_SHARE: f64 = 0.20;

/// Whether copying `size` bytes of `what` fits in `space`: twice the size free, and a fifth of
/// the filesystem still free afterwards. A copy that does not fit is logged as skipped: the
/// worktree's first build or install is then slower, not broken (#419).
pub fn seed_fits(what: &str, size: u64, space: Option<DiskSpace>) -> bool {
    let Some(space) = space else {
        return false;
    };
    let after = space.free.saturating_sub(size);
    let fits = space.free >= size.saturating_mul(2)
        && after as f64 >= SEED_MIN_FREE_SHARE * space.total as f64;
    if !fits {
        let mb = |bytes: u64| bytes / (1024 * 1024);
        tracing::info!(
            what,
            size_mb = mb(size),
            free_mb = mb(space.free),
            total_mb = mb(space.total),
            "🌱 [SEED] skipped: the copy would leave too little disk free"
        );
    }
    fits
}

/// Copies the seed copy's `target/debug` build cache into the new copy at `to`, when there is
/// one and it fits (`seed_fits`). Returns the bytes copied, or `None` when there was nothing to
/// copy or no room for it.
fn seed_build_cache(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<Option<u64>> {
    seed_build_cache_within(from, to, disk_space(to))
}

fn seed_build_cache_within(
    from: &std::path::Path,
    to: &std::path::Path,
    space: Option<DiskSpace>,
) -> std::io::Result<Option<u64>> {
    let source = from.join("target").join("debug");
    let parts: Vec<&str> = SEEDED_BUILD_DIRS
        .iter()
        .copied()
        .filter(|part| source.join(part).is_dir())
        .collect();
    if parts.is_empty() {
        return Ok(None);
    }
    let size: u64 = parts.iter().map(|part| tree_size(&source.join(part))).sum();
    if !seed_fits("target/debug", size, space) {
        return Ok(None);
    }
    let dest = to.join("target").join("debug");
    std::fs::create_dir_all(&dest)?;
    for part in parts {
        // `cp -a` keeps modification times: cargo compares a crate's outputs with those of the
        // crates it depends on, and fresh times in copy order would make half of them stale.
        let status = std::process::Command::new("cp")
            .arg("-a")
            .arg(source.join(part))
            .arg(&dest)
            .status()?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "copying {} failed: {status}",
                source.join(part).display()
            )));
        }
    }
    Ok(Some(size))
}

/// Whether `dir` is a Python virtual environment: it holds `pyvenv.cfg`.
fn is_virtualenv(dir: &std::path::Path) -> bool {
    dir.join("pyvenv.cfg").is_file()
}

/// The `node_modules` trees and Python virtual environments of the copy at `root`, relative to
/// it: at the root and in the packages below it, never one inside another (that is part of its
/// parent), nor any under `.git` or `target`.
fn dependency_trees(root: &std::path::Path) -> Vec<PathBuf> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let path = entry.path();
            match entry.file_name().to_str() {
                Some(".git" | "target") => {}
                Some("node_modules") => found.extend(path.strip_prefix(root).ok().map(Into::into)),
                _ if is_virtualenv(&path) => {
                    found.extend(path.strip_prefix(root).ok().map(Into::into))
                }
                _ => walk(root, &path, found),
            }
        }
    }
    let mut found = Vec::new();
    walk(root, root, &mut found);
    found.sort();
    found
}

/// Copies the seed copy's `node_modules` trees and virtual environments into the new copy at
/// `to`, when there are any and they fit (`seed_fits`, #412, #414, #419). Without them every
/// import from a dependency resolves to nothing in the new
/// worktree until something installs the packages again. `cp -a` keeps the symlinks of `.bin`,
/// of pnpm's layout and of a venv (`lib64 -> lib`, `bin/python`); a venv's scripts are then
/// rewritten to name the copy. The trees are the worktree's own afterwards, so an install in one
/// worktree never changes another's.
fn seed_dependency_trees(
    from: &std::path::Path,
    to: &std::path::Path,
) -> std::io::Result<Option<u64>> {
    seed_dependency_trees_within(from, to, disk_space(to))
}

fn seed_dependency_trees_within(
    from: &std::path::Path,
    to: &std::path::Path,
    space: Option<DiskSpace>,
) -> std::io::Result<Option<u64>> {
    let trees = dependency_trees(from);
    if trees.is_empty() {
        return Ok(None);
    }
    let size: u64 = trees.iter().map(|rel| tree_size(&from.join(rel))).sum();
    if !seed_fits("node_modules and virtual environments", size, space) {
        return Ok(None);
    }
    for rel in trees {
        let dest = to.join(&rel);
        let Some(parent) = dest.parent() else {
            continue;
        };
        std::fs::create_dir_all(parent)?;
        let status = std::process::Command::new("cp")
            .arg("-a")
            .arg(from.join(&rel))
            .arg(parent)
            .status()?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "copying {} failed: {status}",
                from.join(&rel).display()
            )));
        }
        if is_virtualenv(&dest) {
            relocate_virtualenv(&from.join(&rel), &dest)?;
            let _ = prewarm_virtualenv_pycache(&dest);
        }
    }
    Ok(Some(size))
}

/// Rewrites the scripts in the `bin` of a virtual environment copied from `old` to `new` that
/// name `old` (console-script shebangs, `activate`) so that they name `new`: otherwise running
/// the copy's `pytest` would start the other worktree's interpreter (#414). Binaries and files
/// over 1 MiB are left alone. Returns how many scripts were rewritten.
fn relocate_virtualenv(old: &std::path::Path, new: &std::path::Path) -> std::io::Result<usize> {
    let (Some(old_text), Some(new_text)) = (old.to_str(), new.to_str()) else {
        return Ok(0);
    };
    let Ok(entries) = std::fs::read_dir(new.join("bin")) else {
        return Ok(0);
    };
    let mut rewritten = 0;
    for entry in entries.flatten() {
        let small_file = entry.file_type().is_ok_and(|kind| kind.is_file())
            && entry.metadata().is_ok_and(|m| m.len() <= 1 << 20);
        if !small_file {
            continue;
        }
        let path = entry.path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text.contains(old_text) {
            // Writing an existing file keeps its mode, so a script stays executable.
            std::fs::write(&path, text.replace(old_text, new_text))?;
            rewritten += 1;
        }
    }
    Ok(rewritten)
}

/// Finds a trusted host Python interpreter on the system (Roadmap 6.2).
fn find_trusted_host_python() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "/usr/bin/python3",
        "/usr/local/bin/python3",
        "/opt/homebrew/bin/python3",
        "/usr/bin/python",
    ];
    for &cand in CANDIDATES {
        let p = Path::new(cand);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .flat_map(|dir| [dir.join("python3"), dir.join("python")])
            .find(|p| p.is_file())
    })
}

/// Pre-warms Python bytecode (`.pyc` pycache) inside a virtual environment (Roadmap 6.2).
///
/// Uses the host's trusted Python interpreter in isolated mode with site initialization disabled
/// (`-I -S`) to compile all `.py` files in `lib` into bytecode. This eliminates cold import and parse
/// latency without executing untrusted workspace-provided binaries or running untrusted `sitecustomize.py` hooks.
pub fn prewarm_virtualenv_pycache(venv: &std::path::Path) -> std::io::Result<usize> {
    if !is_virtualenv(venv) {
        return Ok(0);
    }
    let lib_dir = venv.join("lib");
    if !lib_dir.is_dir() {
        return Ok(0);
    }
    let host_python = match find_trusted_host_python() {
        Some(p) => p,
        None => return Ok(0),
    };

    match std::process::Command::new(&host_python)
        .args(["-I", "-S", "-m", "compileall", "-q", "-f"])
        .arg(&lib_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => {
            tracing::info!(venv = %venv.display(), "🐍 [PYCACHE] pre-warmed virtual environment bytecode");
            Ok(1)
        }
        Ok(status) => {
            tracing::debug!(venv = %venv.display(), ?status, "compileall completed with non-zero status");
            Ok(0)
        }
        Err(err) => {
            tracing::debug!(venv = %venv.display(), %err, "could not execute host python for compileall; skipping pre-warming");
            Ok(0)
        }
    }
}

/// Bytes of every regular file under `dir`.
fn tree_size(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => tree_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// Free and total bytes of the filesystem that holds `path` (or its nearest existing parent).
pub fn disk_space(path: &std::path::Path) -> Option<DiskSpace> {
    use std::os::unix::ffi::OsStrExt;
    let existing = path.ancestors().find(|p| p.exists())?;
    let c_path = std::ffi::CString::new(existing.as_os_str().as_bytes()).ok()?;
    // SAFETY: an all-zero `statvfs` is a valid value for the call to fill in.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated path and `stat` is valid for writes.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    // The fields are `u64` on Linux and narrower on macOS.
    #[allow(clippy::unnecessary_cast)]
    let (free, total) = (
        stat.f_bavail as u64 * stat.f_frsize as u64,
        stat.f_blocks as u64 * stat.f_frsize as u64,
    );
    Some(DiskSpace { free, total })
}

/// Copies the sources of the seed copy `src` into the new copy `dst`: every per-node cache
/// (`is_node_cache`) stays behind. Some of them hold the seed copy's absolute paths. A CMake
/// `build/` made the new worktree's `check` fail on the old `CMakeCache.txt`, and its
/// `compile_commands.json` pointed clangd at the other copy's sources (#416). The caches that are
/// safe to move are seeded on purpose: `seed_build_cache` and `seed_dependency_trees`.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<usize> {
    let mut copied = 0;
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_node_cache(&name.to_string_lossy()) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if entry.file_type()?.is_symlink() && from.is_dir() {
            // A directory symlink stays one: walking it copied its target a second time, or
            // without end when it points at a parent (#414).
            std::os::unix::fs::symlink(std::fs::read_link(&from)?, &to)?;
        } else if from.is_dir() {
            // A virtual environment is copied whole, its symlinks kept, by
            // `seed_dependency_trees`.
            if !is_virtualenv(&from) {
                copied += copy_tree(&from, &to)?;
            }
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
        || name == workspace::STALE_MARKER
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

/// Whether an LSP message is a request from the server (an id and a method), not a notification.
fn is_server_request(json: &str) -> bool {
    json.contains("\"id\"")
        && serde_json::from_str::<serde_json::Value>(json)
            .is_ok_and(|v| v.get("id").is_some() && v.get("method").is_some())
}

fn fallback_answers_request(json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json).is_ok_and(|value| {
        value.get("id").is_some_and(|id| !id.is_null())
            && matches!(
                value.get("method").and_then(serde_json::Value::as_str),
                Some(
                    "window/workDoneProgress/create"
                        | "workspace/configuration"
                        | "client/registerCapability"
                )
            )
    })
}

/// Sends the client the note an engine attached to an answer given while its server was still
/// loading or indexing, just before the answer, and takes it off the answer (#391).
async fn send_busy_note(resp: &mut serde_json::Value, out_tx: &SharedOutputSender) {
    let Some(busy) = resp
        .as_object_mut()
        .and_then(|o| o.remove(prod_code_protocol::readiness::BUSY_MEMBER))
    else {
        return;
    };
    let note = serde_json::json!({
        "jsonrpc": "2.0",
        "method": prod_code_protocol::readiness::BUSY_NOTIFICATION,
        "params": busy
    });
    let _ = out_tx.send(WireMessage::LspPayload(note.to_string())).await;
}

/// Writes a file a client synced so that a failed write, such as on a full disk, leaves the old
/// content: the text goes to a temporary file next to it, which then replaces it. `fs::write`
/// truncated the file first, and a full disk left it empty (#385).
async fn safe_sync_target(server_workspace: &Path, relative: &str) -> std::io::Result<PathBuf> {
    let invalid_path = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsafe sync path: {relative:?}"),
        )
    };
    let relative_path = Path::new(relative);
    if relative.is_empty() || relative.contains('\\') || relative_path.is_absolute() {
        return Err(invalid_path());
    }
    let components: Vec<&str> = relative.split('/').collect();
    if components
        .iter()
        .any(|component| component.is_empty() || *component == "." || *component == "..")
        || relative_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(invalid_path());
    }

    let mut current = server_workspace.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        current.push(*component);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => {
                if metadata.file_type().is_symlink()
                    || (index + 1 < components.len() && !metadata.is_dir())
                {
                    return Err(invalid_path());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }

    Ok(server_workspace.join(relative_path))
}

async fn write_synced_file(
    target: &std::path::Path,
    content: &[u8],
    executable: bool,
) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = target.with_file_name(format!(
        ".{name}.prod-code-sync-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let written = async {
        tokio::fs::write(&temp, content).await?;
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755)).await?;
        }
        tokio::fs::rename(&temp, target).await
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    written
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
            match tokio::task::spawn_blocking(move || {
                let files = copy_tree(&from, &to)?;
                let started = Instant::now();
                let cache = seed_build_cache(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding the build cache failed");
                    None
                });
                let cache_took = started.elapsed();
                let started = Instant::now();
                let packages = seed_dependency_trees(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding node_modules and virtual environments failed");
                    None
                });
                let packages_took = started.elapsed();
                let started = Instant::now();
                let cpp_cache = cpp_index::seed_cpp_worktree(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding C/C++ clangd index and compilation database failed");
                    None
                });
                let cpp_took = started.elapsed();
                let started = Instant::now();
                let swift_cache = swift_cache::seed_swift_worktree(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding Swift module cache and package checkouts failed");
                    None
                });
                let swift_took = started.elapsed();
                let started = Instant::now();
                let python_cache = python_cache::seed_python_worktree(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding Python virtual-environment stub cache failed");
                    None
                });
                let python_took = started.elapsed();
                let started = Instant::now();
                let ts_cache = ts_cache::seed_typescript_worktree(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding TypeScript type declaration cache and configuration failed");
                    None
                });
                let ts_took = started.elapsed();
                Ok::<_, std::io::Error>((
                    files,
                    cache,
                    cache_took,
                    packages,
                    packages_took,
                    cpp_cache,
                    cpp_took,
                    swift_cache,
                    swift_took,
                    python_cache,
                    python_took,
                    ts_cache,
                    ts_took,
                ))
            })
            .await
            {
                Ok(Ok((
                    files,
                    cache,
                    cache_took,
                    packages,
                    packages_took,
                    cpp_cache,
                    cpp_took,
                    swift_cache,
                    swift_took,
                    python_cache,
                    python_took,
                    ts_cache,
                    ts_took,
                ))) => {
                    seeded = true;
                    tracing::info!(
                        workspace = %target.display(),
                        seed = %seed_dir.display(),
                        files,
                        build_cache_mb = cache.map(|bytes| bytes / (1024 * 1024)),
                        build_cache_ms = cache_took.as_millis() as u64,
                        dependencies_mb = packages.map(|bytes| bytes / (1024 * 1024)),
                        dependencies_ms = packages_took.as_millis() as u64,
                        cpp_cache_mb = cpp_cache.map(|bytes| bytes / (1024 * 1024)),
                        cpp_cache_ms = cpp_took.as_millis() as u64,
                        swift_cache_mb = swift_cache.map(|bytes| bytes / (1024 * 1024)),
                        swift_cache_ms = swift_took.as_millis() as u64,
                        python_cache_kb = python_cache.map(|bytes| bytes / 1024),
                        python_cache_ms = python_took.as_millis() as u64,
                        ts_cache_kb = ts_cache.map(|bytes| bytes / 1024),
                        ts_cache_ms = ts_took.as_millis() as u64,
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
    let (missing, deleted) = tokio::task::spawn_blocking(move || {
        let reconciled = reconcile_manifest(&root, &stamps);
        workspace::forget_stale_paths(&root);
        reconciled
    })
    .await
    .unwrap_or_default();

    if !deleted.is_empty()
        && let Some(ws) = workspace_manager.get_loaded(&target).await
    {
        for engine_lock in ws.mirrored_rust_engines() {
            let mut engine = engine_lock.lock().await;
            for rel in &deleted {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    engine.update_base(&target.join(rel), None)
                }));
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
            ".pnpm-store",
            ".local/share/pnpm",
            ".yarn/cache",
            ".cache/yarn",
            ".local/share/uv",
            ".cache/uv",
            ".cache/pypoetry",
            ".virtualenvs",
            ".pyenv",
            ".local/share/virtualenvs",
            ".local/pipx",
            ".conda",
            ".nvm",
            ".fnm",
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
    if let Some(gopath) = std::env::var_os("GOPATH") {
        let gopath = PathBuf::from(gopath);
        if canonical.starts_with(gopath.join("pkg/mod")) || canonical.starts_with(gopath.join("src")) {
            return true;
        }
    }
    if let Some(goroot) = std::env::var_os("GOROOT") {
        let goroot = PathBuf::from(goroot);
        if canonical.starts_with(&goroot) {
            return true;
        }
    }
    const SYSTEM_ROOTS: [&str; 15] = [
        "/snap",
        "/usr/include",
        "/usr/local/include",
        "/usr/local/Cellar",
        "/usr/lib",
        "/usr/local/lib",
        "/usr/local/go",
        "/usr/share",
        "/opt/homebrew",
        "/Applications/Xcode.app",
        "/Library/Developer",
        "/Library/Frameworks",
        "/System/Library/Frameworks",
        "/opt/conda",
        "/node_modules",
    ];
    if SYSTEM_ROOTS.iter().any(|root| canonical.starts_with(root)) {
        return true;
    }
    canonical.components().any(|c| c.as_os_str() == "node_modules")
}

/// Serves a `ReadFileRequest` under the readable-path policy, capped in size.
fn read_server_file(
    storage_root: &std::path::Path,
    req: &prod_code_protocol::ReadFileRequest,
) -> prod_code_protocol::ReadFileResponse {
    const DEFAULT_MAX_SOURCE: u64 = 2 * 1024 * 1024;
    const MAX_PULL_BYTES: u64 = 64 * 1024 * 1024;
    let path = PathBuf::from(&req.path);
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
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let in_workspace = canonical.starts_with(storage_root);
    let ceiling = if in_workspace {
        MAX_PULL_BYTES
    } else {
        DEFAULT_MAX_SOURCE
    };
    let max = if req.max_bytes == 0 {
        ceiling
    } else {
        req.max_bytes.min(ceiling)
    };
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

    /// The diagnostics for the text last sent for `uri`. A server that answers a pull is asked
    /// for them (the native TypeScript server, and sourcekit-lsp, which does not advertise it);
    /// one that only publishes is waited for until it has published for that text, so a check
    /// of a proposed text is not answered with the errors of the text before it (#293), and a
    /// one-shot session still gets the first publication after its didOpen for quick fixes.
    /// When no publication for that text comes, this is an error, not an empty list (#471).
    async fn diagnostics_for(&self, uri: &str) -> anyhow::Result<Vec<serde_json::Value>> {
        match self {
            ManagedLsp::Go(_) => Ok(Vec::new()),
            ManagedLsp::Generic(engine) => {
                if let Some(items) = engine.pull_diagnostics(uri).await {
                    return Ok(items);
                }
                Ok(engine
                    .current_diagnostics_for(uri, CURRENT_DIAGNOSTICS_WAIT)
                    .await?)
            }
        }
    }
}

/// How long an answer about a document's diagnostics waits for a publishing server to build the
/// text last sent. A C++ translation unit with heavy headers takes seconds on a cold server.
const CURRENT_DIAGNOSTICS_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

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
    // Quick fixes are offered for the diagnostics in the requested range; without them the
    // listing would lack its quick fixes and say nothing of it.
    let diagnostics: Vec<serde_json::Value> = engine
        .diagnostics_for(&uri)
        .await
        .map_err(|e| anyhow::anyhow!("code actions need the document's diagnostics: {e}"))?
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
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rust",
        "go" => "go",
        "py" | "pyi" => "python",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "astro" => "astro",
        "svelte" => "svelte",
        "vue" => "vue",
        "html" | "htm" => "html",
        "css" | "scss" | "sass" | "less" => "css",
        "xml" | "svg" => "xml",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "ixx" => "cpp",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        "swift" => "swift",
        "cs" => "csharp",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "scala" | "sc" => "scala",
        "php" | "phtml" => "php",
        "rb" | "erb" | "rake" | "gemspec" => "ruby",
        "zig" | "zon" => "zig",
        "dart" => "dart",
        "lua" => "lua",
        "hs" | "lhs" => "haskell",
        "ml" | "mli" => "ocaml",
        "ex" | "exs" => "elixir",
        "clj" | "cljs" | "cljc" | "edn" => "clojure",
        "jl" => "julia",
        "r" | "R" | "Rmd" => "r",
        "erl" | "hrl" => "erlang",
        "pl" | "pm" => "perl",
        "sol" => "solidity",
        "nim" | "nims" | "nimble" => "nim",
        "d" | "di" => "d",
        "f" | "for" | "f90" | "f95" | "f03" | "f08" => "fortran",
        "cr" => "crystal",
        "groovy" | "gvy" | "gy" | "gsh" | "gradle" => "groovy",
        "adb" | "ads" => "ada",
        "v" | "vh" => "v",
        "rkt" => "racket",
        "tf" | "tfvars" => "terraform",
        "nix" => "nix",
        "s" | "S" | "asm" => "assembly",
        "sql" => "sql",
        "graphql" | "gql" => "graphql",
        "proto" => "proto",
        "thrift" => "thrift",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" => "markdown",
        "sh" | "bash" | "zsh" => "shellscript",
        _ if name == "CMakeLists.txt" || path.extension().and_then(|e| e.to_str()) == Some("cmake") => "cmake",
        _ if name == "Dockerfile" || name == "Containerfile" => "dockerfile",
        _ if name == "Makefile" || name == "makefile" || name == "GNUmakefile" => "makefile",
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
            let path = uri_or_path(&uri);
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
        "uri": file_uri(&item.path),
        "range": lsp_range(item.line, item.col, item.end_line, item.end_col),
        "selectionRange": lsp_range(item.line, item.col, item.line, item.col + item.name.chars().count() as u32),
    })
}

/// Call hierarchy and implementation queries on the in-memory Rust engine, in LSP shape.
fn hierarchy_query(
    snapshot: &prod_code_engine_rust::RustEngineSnapshot,
    method: &str,
    path: &std::path::Path,
    line: u32,
    col: u32,
) -> anyhow::Result<serde_json::Value> {
    Ok(match method {
        "textDocument/prepareCallHierarchy" => serde_json::Value::Array(
            snapshot
                .prepare_call_hierarchy(path, line, col)?
                .iter()
                .map(hierarchy_item_json)
                .collect(),
        ),
        "callHierarchy/incomingCalls" | "callHierarchy/outgoingCalls" => {
            let incoming = method == "callHierarchy/incomingCalls";
            let edges = if incoming {
                snapshot.incoming_calls(path, line, col)?
            } else {
                snapshot.outgoing_calls(path, line, col)?
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
            let items: Vec<serde_json::Value> = snapshot
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
            snapshot
                .goto_implementation(path, line, col)?
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "uri": file_uri(&t.path),
                        "range": lsp_range(t.line, t.col, t.line, t.col),
                    })
                })
                .collect(),
        ),
        other => anyhow::bail!("unsupported hierarchy method {other}"),
    })
}

/// Maximum duration a semantic read query (references, definition, hover, call hierarchy,
/// symbols) may run before being terminated with a server-side timeout error.
const SEMANTIC_QUERY_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum transparent retries when a snapshot query is cancelled by a concurrent mutation
/// (e.g. an edit or sync mutating the Salsa database).
const MAX_CANCELLATION_RETRIES: usize = 3;

/// Maximum concurrent semantic read queries executing in the blocking pool (#3111).
const MAX_CONCURRENT_SEMANTIC_QUERIES: usize = 16;

/// Global semaphore bounding concurrent semantic queries to strictly limit the number
/// of outstanding blocking query tasks, preventing exhaustion of Tokio's blocking pool (#3111).
static SEMANTIC_QUERY_SEMAPHORE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SEMANTIC_QUERIES)));

/// Counter of currently active queries that have timed out and are still running in the background.
static STALLED_QUERIES_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Executes a read query on a thread-safe snapshot of `RustEngine`.
///
/// Holds the engine lock only briefly (<10 µs) to activate session overlays and take
/// an `Analysis` snapshot. The query then runs on a blocking thread pool worker without
/// retaining the shared engine lock, bounded by `SEMANTIC_QUERY_TIMEOUT`.
///
/// Bounded by `SEMANTIC_QUERY_SEMAPHORE` so outstanding blocking queries cannot accumulate
/// or exhaust Tokio's blocking pool. If timed-out queries accumulate and threaten capacity,
/// the engine is cooperatively recycled to purge stuck workers while active queries transparently
/// retry on fresh snapshots (#3109, #3111).
async fn execute_bounded_query<T, F>(
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
    session_id: u64,
    file_path: &Path,
    is_single_owner: bool,
    query_fn: F,
) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: Fn(&prod_code_engine_rust::RustEngineSnapshot) -> anyhow::Result<T> + Clone + Send + 'static,
{
    let deadline = Instant::now() + SEMANTIC_QUERY_TIMEOUT;
    let mut retries = 0;

    let now = Instant::now();
    if now >= deadline {
        return Err(anyhow::anyhow!(
            "query timed out after {}s",
            SEMANTIC_QUERY_TIMEOUT.as_secs()
        ));
    }
    let remaining = deadline - now;

    let permit = match tokio::time::timeout(
        remaining,
        SEMANTIC_QUERY_SEMAPHORE.clone().acquire_owned(),
    )
    .await
    {
        Ok(Ok(permit)) => permit,
        Ok(Err(_closed)) => anyhow::bail!("semantic query semaphore closed"),
        Err(_elapsed) => {
            if STALLED_QUERIES_COUNT.load(Ordering::Relaxed) > 0 {
                let engine_for_recycle = Arc::clone(engine_lock);
                tokio::task::spawn(async move {
                    if let Ok(mut engine) =
                        tokio::time::timeout(Duration::from_secs(2), engine_for_recycle.lock()).await
                    {
                        engine.trigger_cancellation();
                    }
                });
            }
            anyhow::bail!("query timed out waiting for available execution slot");
        }
    };

    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(anyhow::anyhow!(
                "query timed out after {}s",
                SEMANTIC_QUERY_TIMEOUT.as_secs()
            ));
        }
        let remaining = deadline - now;

        let snapshot = match tokio::time::timeout(remaining, engine_lock.lock()).await {
            Ok(mut engine) => {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                if file_path.is_dir() {
                    engine.snapshot_for(file_path)
                } else {
                    engine.snapshot_for_path(file_path)
                }
            }
            Err(_elapsed) => {
                return Err(anyhow::anyhow!(
                    "query timed out after {}s waiting for engine lock",
                    SEMANTIC_QUERY_TIMEOUT.as_secs()
                ));
            }
        };

        let q_fn = query_fn.clone();
        let mut query_task = Box::pin(tokio::task::spawn_blocking(move || {
            let panic_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| q_fn(&snapshot)));
            match panic_res {
                Ok(res) => res,
                Err(panic_payload) => {
                    let msg = panic_message(panic_payload);
                    Err(anyhow::anyhow!("analyzer panic: {msg}"))
                }
            }
        }));

        let now = Instant::now();
        if now >= deadline {
            let stalled = STALLED_QUERIES_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            let engine_for_recycle = Arc::clone(engine_lock);
            tokio::task::spawn(async move {
                let _held_permit = permit;
                if stalled >= (MAX_CONCURRENT_SEMANTIC_QUERIES / 2) {
                    tracing::warn!(
                        stalled,
                        "Stalled semantic queries reached threshold; recycling engine to reclaim blocking pool workers"
                    );
                    if let Ok(mut engine) =
                        tokio::time::timeout(Duration::from_secs(2), engine_for_recycle.lock()).await
                    {
                        engine.trigger_cancellation();
                    }
                }
                let _ = query_task.await;
                STALLED_QUERIES_COUNT.fetch_sub(1, Ordering::Relaxed);
            });
            return Err(anyhow::anyhow!(
                "query timed out after {}s",
                SEMANTIC_QUERY_TIMEOUT.as_secs()
            ));
        }
        let remaining = deadline - now;

        match tokio::time::timeout(remaining, query_task.as_mut()).await {
            Ok(Ok(Ok(val))) => return Ok(val),
            Ok(Ok(Err(err))) => {
                if prod_code_engine_rust::is_salsa_cancelled(&err)
                    && retries < MAX_CANCELLATION_RETRIES
                {
                    let backoff = Duration::from_millis(5 * (retries + 1) as u64);
                    if Instant::now() + backoff < deadline {
                        retries += 1;
                        tracing::debug!(
                            session = session_id,
                            file = %file_path.display(),
                            attempt = retries,
                            "Salsa query was cancelled by concurrent mutation; retrying with fresh snapshot"
                        );
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                }
                return Err(err);
            }
            Ok(Err(join_err)) => return Err(anyhow::anyhow!("native query task failed: {join_err}")),
            Err(_elapsed) => {
                // The query timed out. Retain the permit while the detached task continues,
                // so that running timed-out tasks count against the concurrency bound and
                // cannot accumulate beyond MAX_CONCURRENT_SEMANTIC_QUERIES (#3111).
                let stalled = STALLED_QUERIES_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                let engine_for_recycle = Arc::clone(engine_lock);
                tokio::task::spawn(async move {
                    let _held_permit = permit;
                    if stalled >= (MAX_CONCURRENT_SEMANTIC_QUERIES / 2) {
                        tracing::warn!(
                            stalled,
                            "Stalled semantic queries reached threshold; recycling engine to reclaim blocking pool workers"
                        );
                        if let Ok(mut engine) =
                            tokio::time::timeout(Duration::from_secs(2), engine_for_recycle.lock()).await
                        {
                            engine.trigger_cancellation();
                        }
                    }
                    let _ = query_task.await;
                    STALLED_QUERIES_COUNT.fetch_sub(1, Ordering::Relaxed);
                });
                return Err(anyhow::anyhow!(
                    "query timed out after {}s",
                    SEMANTIC_QUERY_TIMEOUT.as_secs()
                ));
            }
        }
    }
}

/// Builds an LSP `WorkspaceEdit` (as `documentChanges`) from a refactoring outcome: new files
/// become create operations followed by their content, every rewritten file one whole-file text
/// edit, and file moves rename operations. The analyzer names every path as it was before the
/// refactoring, and LSP applies `documentChanges` in order, so the moves come last, as
/// rust-analyzer's own server sends them: a rewrite after them would name a path a move vacated
/// or gave to another file.
fn workspace_edit_json(outcome: &prod_code_engine_rust::RefactorOutcome) -> serde_json::Value {
    let mut changes = Vec::new();
    for created in &outcome.created {
        let uri = file_uri(&created.path);
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
            "textDocument": { "uri": file_uri(&file.path), "version": null },
            "edits": [ {
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": file.old_line_count, "character": 0 } },
                "newText": file.new_text
            } ]
        }));
    }
    for mv in &outcome.moves {
        changes.push(serde_json::json!({
            "kind": "rename",
            "oldUri": file_uri(&mv.from),
            "newUri": file_uri(&mv.to),
            "options": { "overwrite": false }
        }));
    }
    serde_json::json!({ "documentChanges": changes })
}

/// Default wall-clock limit for a remote command when the client does not set one.
const EXEC_DEFAULT_TIMEOUT_SECS: u64 = 3600;

/// Maximum supported timeout for remote execution (7 days) to prevent overflow in deadline arithmetic.
const MAX_REMOTE_EXEC_TIMEOUT_SECS: u64 = 86400 * 7;

/// Maximum line buffer size for parsing structured JSON/text test streams (1 MiB).
const MAX_JSON_LINE_BUFFER_BYTES: usize = 1024 * 1024;

/// Largest file whose old bytes a pre-command snapshot keeps, so that the gateway can put it
/// back when the command's changes never reach the client (#262). Source files are far smaller;
/// what is larger is mostly data a command regenerates anyway.
const RESTORE_MAX_FILE: u64 = 1024 * 1024;

/// Most bytes one pre-command snapshot keeps in memory. A command runs while its snapshot is
/// held, so a checkout with a large tree of small files must not cost the node gigabytes; a
/// file past the budget is marked stale when it has to be restored, and the client sends it.
const RESTORE_BUDGET: u64 = 256 * 1024 * 1024;

/// The old contents of a file, kept to restore it.
struct KeptFile {
    bytes: Vec<u8>,
    #[cfg(unix)]
    mode: u32,
}

/// What a workspace copy held before a remote command ran.
#[derive(Default)]
struct TreeSnapshot {
    /// Size and content hash of every file, for detecting what the command changed.
    stamps: std::collections::HashMap<String, (u64, u64)>,
    /// The bytes of every file up to [`RESTORE_MAX_FILE`], within [`RESTORE_BUDGET`].
    kept: std::collections::HashMap<String, KeptFile>,
}

/// Size and content hash of every file under `root` the sync layer cares about (build output
/// and VCS internals excluded).
fn stamp_tree(root: &std::path::Path) -> std::collections::HashMap<String, (u64, u64)> {
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

/// [`stamp_tree`] plus the bytes of the files small enough to keep, for detecting what a remote
/// command changed and for undoing it when the client cannot receive the changes.
fn snapshot_tree(root: &std::path::Path) -> TreeSnapshot {
    snapshot_tree_within(root, RESTORE_MAX_FILE, RESTORE_BUDGET)
}

/// [`snapshot_tree`] with its limits given, so that a test can exceed them cheaply.
fn snapshot_tree_within(root: &std::path::Path, max_file: u64, mut budget: u64) -> TreeSnapshot {
    let mut present = Vec::new();
    walk_files(root, root, &mut present);
    // Walked in a fixed order, so which files fit the budget does not depend on the directory
    // listing order.
    present.sort();
    let mut snapshot = TreeSnapshot::default();
    for (rel, path) in present {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let size = bytes.len() as u64;
        snapshot
            .stamps
            .insert(rel.clone(), (size, content_hash(&bytes)));
        if size > max_file || size > budget {
            continue;
        }
        budget -= size;
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&path)
                .map(|m| m.permissions().mode())
                .unwrap_or(0o644)
        };
        snapshot.kept.insert(
            rel,
            KeptFile {
                bytes,
                #[cfg(unix)]
                mode,
            },
        );
    }
    snapshot
}

/// What [`restore_tree`] did to a workspace copy.
#[derive(Debug, Default)]
struct Restored {
    /// Every file put back as it was, created by the command and removed, or removed because
    /// its old bytes were not kept: what a warm engine must be told.
    files: Vec<FileDelta>,
    /// The files among them that could not be put back and are gone from the copy.
    stale: Vec<String>,
}

/// Undoes what a command changed in the workspace copy since `before`: a changed file gets its
/// old bytes back, a file the command created is removed, a file it deleted is recreated. A
/// changed or deleted file whose old bytes were not kept is removed and reported as stale, so
/// that the client sends its own version. The comparison is made here rather than with
/// [`changed_since`], which leaves out files above the size the client is sent: a restore must
/// not leave any of them behind.
///
/// `synced` holds the files a client sync delivered while the command ran, with the hash of the
/// text it wrote. Such a file is the checkout's newer text, not the command's, and stays; if the
/// command changed it again after it arrived, that text is gone, so the file is removed and
/// reported as stale like one whose bytes were not kept.
fn restore_tree(
    root: &std::path::Path,
    before: &TreeSnapshot,
    synced: &std::collections::HashMap<String, Option<u64>>,
) -> Restored {
    let after = stamp_tree(root);
    let mut restored = Restored::default();
    let mut touched: Vec<&String> = after
        .iter()
        .filter(|(rel, stamp)| before.stamps.get(*rel) != Some(*stamp))
        .map(|(rel, _)| rel)
        .chain(before.stamps.keys().filter(|rel| !after.contains_key(*rel)))
        .collect();
    touched.sort();
    for rel in touched {
        let target = root.join(rel);
        if let Some(synced_hash) = synced.get(rel) {
            if after.get(rel).map(|stamp| stamp.1) == *synced_hash {
                continue;
            }
            if target.exists() && std::fs::remove_file(&target).is_err() {
                continue;
            }
            prune_empty_parents(root, target.parent());
            restored.stale.push(rel.clone());
            restored.files.push(FileDelta {
                relative_path: rel.clone(),
                content: None,
                is_executable: false,
            });
            continue;
        }
        match before.kept.get(rel) {
            Some(kept) => {
                if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&target, &kept.bytes) {
                    tracing::warn!(error = %e, file = %target.display(), "restoring a file failed");
                    continue;
                }
                #[cfg(unix)]
                let is_executable = {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &target,
                        std::fs::Permissions::from_mode(kept.mode),
                    );
                    kept.mode & 0o111 != 0
                };
                #[cfg(not(unix))]
                let is_executable = false;
                restored.stale.push(rel.clone());
                restored.files.push(FileDelta {
                    relative_path: rel.clone(),
                    content: Some(kept.bytes.clone()),
                    is_executable,
                });
            }
            None => {
                if target.exists() && std::fs::remove_file(&target).is_err() {
                    continue;
                }
                prune_empty_parents(root, target.parent());
                if before.stamps.contains_key(rel) {
                    restored.stale.push(rel.clone());
                }
                restored.files.push(FileDelta {
                    relative_path: rel.clone(),
                    content: None,
                    is_executable: false,
                });
            }
        }
    }
    restored
}

/// Puts the workspace copy back the way `before` found it when the changes a command made will
/// never reach its client (#262), and returns how many files were put back. Without this the
/// copy keeps changes the checkout does not have: the next sync sends only what changed locally,
/// so a later check would run on code nobody committed. The files that could not be put back
/// are recorded as stale for the next handshake or sync. The warm engines are told, like after
/// a sync, so that they see the old text again.
async fn restore_after_lost_client(
    workspace_manager: &WorkspaceManager,
    workspace: &std::path::Path,
    before: Arc<TreeSnapshot>,
    started: Instant,
) -> usize {
    let root = workspace.to_path_buf();
    let restored = tokio::task::spawn_blocking(move || {
        let synced = workspace::synced_since(&root, started);
        let restored = restore_tree(&root, &before, &synced);
        workspace::record_stale_paths(&root, &restored.stale);
        restored
    })
    .await
    .unwrap_or_default();
    let unkept = restored
        .files
        .iter()
        .filter(|f| f.content.is_none())
        .count();
    if unkept > 0 {
        tracing::warn!(
            workspace = %workspace.display(),
            stale = unkept,
            "🛠️ [EXEC] files a command changed were too large to keep; removed until the client sends them"
        );
    }
    refresh_engines(workspace_manager, workspace, &restored.files).await;
    restored
        .files
        .iter()
        .filter(|f| f.content.is_some())
        .count()
}

/// Files that differ between `before` and the tree now: new/changed ones with content,
/// removed ones as deletions. Files above 5 MiB are ignored.
fn changed_since(
    root: &std::path::Path,
    before: &std::collections::HashMap<String, (u64, u64)>,
) -> Vec<FileDelta> {
    const MAX_PULL_FILE: u64 = 5 * 1024 * 1024;
    let after = stamp_tree(root);
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
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.update_base(&target, text.clone())
            }));
            match res {
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, file = %target.display(), "engine update after a command failed");
                }
                Err(panic) => {
                    let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    tracing::warn!(panic = %msg, file = %target.display(), "engine update after a command panicked; continuing");
                }
                Ok(Ok(())) => {}
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
/// The environment that lets every git worktree of a project share one C/C++ compiler cache
/// (#243). Each worktree has its own server copy at its own path, and a compiler cache keyed by
/// absolute paths shares nothing between them: the fmt library built in a second copy took
/// 22.9 s with sccache as the launcher (2 hits of 114 compiles) and 0.71 s with ccache and
/// `CCACHE_BASEDIR` set to the copy, which ccache reads on every compile. sccache reads its
/// base directories once, when its server starts, so it cannot follow worktrees that appear
/// later. CMake picks the launchers up when it configures a build directory. Nothing when the
/// node has no ccache; the caller's own variables are applied after these and win.
pub fn compiler_cache_env(workspace: &Path, ccache: bool) -> Vec<(String, String)> {
    if !ccache {
        return Vec::new();
    }
    vec![
        (
            "CCACHE_BASEDIR".to_string(),
            workspace.to_string_lossy().into_owned(),
        ),
        ("CCACHE_NOHASHDIR".to_string(), "1".to_string()),
        (
            "CCACHE_SLOPPINESS".to_string(),
            "pch_defines,time_macros".to_string(),
        ),
        ("CCACHE_PCH_EXTERNAL_CHECKS".to_string(), "1".to_string()),
        (
            "CMAKE_C_COMPILER_LAUNCHER".to_string(),
            "ccache".to_string(),
        ),
        (
            "CMAKE_CXX_COMPILER_LAUNCHER".to_string(),
            "ccache".to_string(),
        ),
    ]
}

/// Polyglot compiler and build cache environment across Rust, Go, Python, Node, C/C++, and Swift (Roadmap 6.2, 3.4, 3.7).
pub fn polyglot_compiler_cache_env(
    workspace: &Path,
    ccache: bool,
    ram_target_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env = compiler_cache_env(workspace, ccache);
    if let Some(target) = ram_target_dir {
        env.push((
            "CARGO_TARGET_DIR".to_string(),
            target.to_string_lossy().into_owned(),
        ));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home_path = PathBuf::from(home);
        let gocache = home_path.join(".cache/go-build");
        let gomodcache = home_path.join("go/pkg/mod");
        if gocache.is_dir() {
            env.push(("GOCACHE".to_string(), gocache.to_string_lossy().into_owned()));
        }
        if gomodcache.is_dir() {
            env.push(("GOMODCACHE".to_string(), gomodcache.to_string_lossy().into_owned()));
        }
        let uv_cache = home_path.join(".cache/uv");
        if uv_cache.is_dir() {
            env.push(("UV_CACHE_DIR".to_string(), uv_cache.to_string_lossy().into_owned()));
        }
        let pip_cache = home_path.join(".cache/pip");
        if pip_cache.is_dir() {
            env.push(("PIP_CACHE_DIR".to_string(), pip_cache.to_string_lossy().into_owned()));
        }
        let pnpm_store = home_path.join(".local/share/pnpm/store");
        if pnpm_store.is_dir() {
            env.push(("npm_config_store_dir".to_string(), pnpm_store.to_string_lossy().into_owned()));
        }
        let npm_cache = home_path.join(".npm");
        if npm_cache.is_dir() {
            env.push(("npm_config_cache".to_string(), npm_cache.to_string_lossy().into_owned()));
        }
        let yarn_cache = home_path.join(".cache/yarn");
        if yarn_cache.is_dir() {
            env.push(("YARN_CACHE_FOLDER".to_string(), yarn_cache.to_string_lossy().into_owned()));
        }
    }
    // Python shared virtual-environment stub cache across worktrees (Roadmap 3.6)
    env.extend(python_cache::python_stub_cache_env());
    // Swift shared module cache across worktrees (Roadmap 3.7)
    env.extend(swift_cache::swift_module_cache_env());
    // TypeScript shared @types and declaration cache across worktrees (Roadmap 3.5)
    env.extend(ts_cache::ts_types_cache_env());
    env
}

pub fn is_ram_cache_enabled_with(enabled: bool, env_val: Option<&str>) -> bool {
    enabled
        || env_val
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false)
}

pub fn is_ram_cache_enabled(enabled: bool) -> bool {
    let env = std::env::var("PROD_CODE_BUILD_RAM").ok();
    is_ram_cache_enabled_with(enabled, env.as_deref())
}

/// Resolves or initializes an isolated in-memory RAM-disk build cache for `workspace` (Roadmap 6.2).
/// Returns `Some(PathBuf)` if enabled and headroom permits (>= 20% free and >= 256 MiB free); otherwise `None`.
pub fn resolve_ram_build_cache(
    workspace: &Path,
    enabled: bool,
    custom_dir: Option<&Path>,
) -> Option<PathBuf> {
    if !is_ram_cache_enabled(enabled) {
        return None;
    }
    let default_shm = Path::new("/dev/shm/prod-code-build");
    let fallback_tmp = Path::new("/tmp/prod-code-build");
    let base_dir = custom_dir.unwrap_or_else(|| {
        if Path::new("/dev/shm").is_dir() {
            default_shm
        } else {
            fallback_tmp
        }
    });

    if let Some(space) = disk_space(base_dir) {
        let free_share = space.free as f64 / space.total.max(1) as f64;
        const MIN_FREE_RAM_SHARE: f64 = 0.20;
        const MIN_FREE_BYTES: u64 = 256 * 1024 * 1024;
        if free_share < MIN_FREE_RAM_SHARE || space.free < MIN_FREE_BYTES {
            tracing::info!(
                dir = %base_dir.display(),
                free_mb = space.free / (1024 * 1024),
                "🌱 [BUILD_RAM] insufficient RAM disk headroom; falling back to disk cache"
            );
            return None;
        }
    }

    let workspace_name = workspace.file_name()?.to_str()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&workspace, &mut hasher);
    let hash = std::hash::Hasher::finish(&hasher);
    let ws_cache_dir = base_dir.join(format!("{workspace_name}-{hash:016x}"));
    let target_dir = ws_cache_dir.join("target");
    if let Err(e) = std::fs::create_dir_all(&target_dir) {
        tracing::warn!(%e, dir = %target_dir.display(), "failed to create RAM build cache dir; falling back to disk");
        return None;
    }
    let marker = ws_cache_dir.join(".last_used");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&marker);
    Some(target_dir)
}

/// An RAII lease that marks a RAM-disk cache directory as actively in use by a running build.
///
/// On Unix, an advisory flock is held on the marker file for the entire lifetime of the lease.
/// If the gateway process is killed or crashes, the OS kernel automatically closes the file
/// descriptor and releases the lock, allowing sweepers to identify and clean stale markers.
pub struct RamBuildLease {
    marker: Option<PathBuf>,
    #[cfg(unix)]
    _lock_file: Option<std::fs::File>,
}

impl RamBuildLease {
    pub fn acquire(target_dir: &Path) -> Self {
        if let Some(ws_cache_dir) = target_dir.parent() {
            let lease_id = NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let marker = ws_cache_dir.join(format!(".active_{}_{}", pid, lease_id));
            #[cfg(unix)]
            {
                use std::io::Write;
                use std::os::unix::io::AsRawFd;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&marker)
                {
                    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                    if ret == 0 {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let meta = format!(
                            "{{\"pid\":{},\"lease_id\":{},\"created_at\":{}}}\n",
                            pid, lease_id, now
                        );
                        let _ = file.write_all(meta.as_bytes());
                        let _ = file.flush();
                        return Self {
                            marker: Some(marker),
                            _lock_file: Some(file),
                        };
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let meta = format!(
                    "{{\"pid\":{},\"lease_id\":{},\"created_at\":{}}}\n",
                    pid, lease_id, now
                );
                if let Ok(()) = std::fs::write(&marker, meta.as_bytes()) {
                    return Self { marker: Some(marker) };
                }
            }
        }
        Self {
            marker: None,
            #[cfg(unix)]
            _lock_file: None,
        }
    }
}

impl Drop for RamBuildLease {
    fn drop(&mut self) {
        if let Some(ref path) = self.marker {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Checks whether a RAM-disk lease marker represents an active build process.
///
/// On Unix, an advisory flock is held for the lifetime of a live lease. If the process has died or crashed,
/// flock acquisition succeeds; this function unlinks the stale marker and returns `false`.
/// If the lock cannot be acquired because a running process is holding it, returns `true`.
pub fn is_ram_lease_active(marker: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(marker)
        {
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if ret == 0 {
                // Successfully locked: owning process died or exited without dropping the lease.
                // Remove the stale marker file while holding the lock.
                let _ = std::fs::remove_file(marker);
                false
            } else {
                // Lock busy: active build process holds this lease.
                true
            }
        } else {
            // Already unlinked or cannot open
            false
        }
    }
    #[cfg(not(unix))]
    {
        if let Ok(meta) = marker.metadata() {
            if let Ok(elapsed) = meta.modified().and_then(|m| m.elapsed()) {
                if elapsed.as_secs() > 7200 {
                    let _ = std::fs::remove_file(marker);
                    return false;
                }
            }
        }
        true
    }
}

/// Sweeps stale or orphaned RAM-disk build caches on startup or periodic maintenance.
pub fn sweep_ram_build_caches(base_dir: &Path) -> usize {
    if !base_dir.is_dir() {
        return 0;
    }
    let running: std::collections::HashSet<String> = RUNNING_COMMANDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .map(|(ws, _, _)| ws.clone())
        .collect();

    let mut removed = 0;
    if let Ok(entries) = std::fs::read_dir(base_dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let path = entry.path();
                let dir_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                let is_running = running.iter().any(|ws| dir_name.starts_with(ws));
                if is_running {
                    continue;
                }
                if let Ok(children) = std::fs::read_dir(&path) {
                    let mut has_active_lease = false;
                    for child in children.flatten() {
                        let name = child.file_name();
                        let name_str = name.to_str().unwrap_or_default();
                        if name_str.starts_with(".active_") {
                            if is_ram_lease_active(&child.path()) {
                                has_active_lease = true;
                            }
                        }
                    }
                    if has_active_lease {
                        continue;
                    }
                }
                let marker = path.join(".last_used");
                let metadata_target = if marker.is_file() {
                    marker.metadata().ok()
                } else {
                    entry.metadata().ok()
                };
                if let Some(meta) = metadata_target {
                    let is_old = meta
                        .modified()
                        .ok()
                        .and_then(|m| m.elapsed().ok())
                        .map(|age| age.as_secs() > 86400)
                        .unwrap_or(false);
                    if is_old {
                        if let Ok(()) = std::fs::remove_dir_all(&path) {
                            removed += 1;
                        }
                    }
                }
            }
        }
    }
    if removed > 0 {
        tracing::info!(dir = %base_dir.display(), removed, "swept old RAM-disk build caches");
    }
    removed
}

/// Whether `program` is an executable file in a directory of `PATH`.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

pub async fn run_exec(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: ExecRequest,
) -> Result<()> {
    run_exec_with_ram(
        storage_root,
        metrics,
        workspace_manager,
        framed,
        req,
        false,
        None,
    )
    .await
}

pub async fn run_exec_with_ram(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: ExecRequest,
    build_cache_ram: bool,
    build_cache_dir: Option<&std::path::Path>,
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
    workspace::touch_last_used(&workspace);
    let Some((program, args)) = req.command.split_first() else {
        framed
            .send(WireMessage::ExecExit(fail("empty command".to_string())))
            .await?;
        return Ok(());
    };

    let timeout_secs = if req.timeout_secs == 0 {
        EXEC_DEFAULT_TIMEOUT_SECS
    } else {
        req.timeout_secs
    };
    if timeout_secs > MAX_REMOTE_EXEC_TIMEOUT_SECS {
        framed
            .send(WireMessage::ExecExit(fail(format!(
                "timeout_secs ({timeout_secs}) exceeds maximum allowed ({MAX_REMOTE_EXEC_TIMEOUT_SECS})"
            ))))
            .await?;
        return Ok(());
    }
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let deadline = match tokio::time::Instant::now().checked_add(timeout) {
        Some(d) => d,
        None => {
            framed
                .send(WireMessage::ExecExit(fail(
                    "timeout_secs overflowed deadline calculation".to_string(),
                )))
                .await?;
            return Ok(());
        }
    };
    if let Some((free, total)) = workspace::free_and_total_bytes(&workspace)
        .or_else(|| workspace::free_and_total_bytes(storage_root))
    {
        let free_gb = free as f64 / (1024.0 * 1024.0 * 1024.0);
        let used_pct = if total > 0 {
            (1.0 - (free as f64 / total as f64)) * 100.0
        } else {
            0.0
        };
        let hostname = get_hostname();
        if free_gb < 5.0 || used_pct > 95.0 {
            framed
                .send(WireMessage::ExecExit(fail(format!(
                    "refused: node {hostname} has only {free_gb:.1} GB free on {} ({used_pct:.0}% full)",
                    workspace.display()
                ))))
                .await?;
            return Ok(());
        }
        if free_gb < 10.0 {
            let warn = format!(
                "[prod-code exec] WARNING: {free_gb:.1} GB free on node {hostname} ({used_pct:.0}% used)\n"
            );
            let _ = framed
                .send(WireMessage::ExecChunk(ExecChunk {
                    stderr: true,
                    data: Some(warn.into_bytes()),
                }))
                .await;
        }
    }
    // Syncs that land after this are the client's newer text, which a restore leaves alone.
    let snapshot_started = Instant::now();
    let before = Arc::new(if req.pull_changes {
        let root = workspace.clone();
        tokio::task::spawn_blocking(move || snapshot_tree(&root))
            .await
            .unwrap_or_default()
    } else {
        TreeSnapshot::default()
    });

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
    // status (#180); tokio only gets the pipes. The gateway binary starts it through its exec
    // shim, so that the peak memory reported is the command's and not the gateway's (#255).
    let ram_target = resolve_ram_build_cache(&workspace, build_cache_ram, build_cache_dir);
    let _ram_lease = ram_target.as_deref().map(RamBuildLease::acquire);
    let (mut cmd, report) = exec_shim::command(program);
    cmd.args(args)
        .current_dir(&run_dir)
        .envs(polyglot_compiler_cache_env(&workspace, on_path("ccache"), ram_target.as_deref()))
        .envs(req.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // The cluster's secret credentials (token and TLS material) are never given to executed commands (#402, Phase 5.6).
    // Scrubbed AFTER request env is applied so that client requests cannot inject or read cluster secrets.
    cmd.scrub_cluster_secrets();
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
    let _running = RunningEntry::start(&workspace, &req.command);

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

    let mut timed_out = false;
    let mut status = None;
    let mut chunks_open = true;
    let mut client_left = false;
    loop {
        tokio::select! {
            chunk = rx.recv(), if chunks_open => match chunk {
                // A client that can no longer be written to is gone as surely as one that
                // hung up, and the command must not outlive it either way.
                Ok(chunk) => client_left = framed.send(WireMessage::ExecChunk(chunk)).await.is_err(),
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
                Some(Ok(WireMessage::Ping)) => {
                    client_left = framed.send(WireMessage::Pong).await.is_err();
                }
                // A connection that fails is as gone as one that closed: nothing the command
                // changes can reach the client anymore.
                Some(Ok(WireMessage::Disconnect { .. })) | Some(Err(_)) | None => client_left = true,
                _ => {}
            }
        }
        if client_left || (!chunks_open && status.is_some()) {
            break;
        }
    }
    if client_left {
        kill_exec_group(pid, &exited);
        // Readers blocked on a full channel see it close and let go of the pipes.
        drop(rx);
        // The command has to be gone before its changes are undone, or it could write again
        // after the restore.
        if status.is_none() {
            let _ = exit_rx.await;
        }
        if req.pull_changes {
            let restored =
                restore_after_lost_client(workspace_manager, &workspace, before, snapshot_started)
                    .await;
            tracing::info!(
                workspace = %workspace_str,
                "🛠️ [EXEC] client left; command killed; {restored} file(s) it changed restored"
            );
        } else {
            tracing::info!(workspace = %workspace_str, "🛠️ [EXEC] client left; command killed");
        }
        return Ok(());
    }
    for reader in readers {
        let _ = reader.await;
    }
    // The shim's report describes the command itself. There is none when the group was killed
    // on a timeout, and then what `wait4` said about the shim stands in for it.
    let report_status = report.as_ref().and_then(exec_shim::ReportFile::read);
    let shim_raw_status = status.flatten();
    let had_report = report.is_some();
    drop(report);
    let (exit_code, usage, exec_err) = if let Some((raw, usage)) = report_status {
        use std::os::unix::process::ExitStatusExt;
        (std::process::ExitStatus::from_raw(raw).code(), Some(usage), None)
    } else if let Some((raw, usage)) = shim_raw_status {
        use std::os::unix::process::ExitStatusExt;
        let exit_status = std::process::ExitStatus::from_raw(raw);
        if exit_status.code() == Some(74) && had_report && !timed_out {
            (
                Some(74),
                Some(usage),
                Some("exec shim failed to write process report (disk full or write error)".to_string()),
            )
        } else {
            (exit_status.code(), Some(usage), None)
        }
    } else {
        (None, None, None)
    };
    if exit_code == Some(254) {
        tracing::warn!("exec command exited with 254; ensuring sccache server is running cleanly on host");
        tokio::task::spawn_blocking(crate::shadow::ensure_sccache_server).await.ok();
    }
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
        ev.ok = exit_code == Some(0) && exec_err.is_none();
        metrics.record(ev);
    }
    if req.pull_changes {
        let (root, snapshot) = (workspace.clone(), Arc::clone(&before));
        let files = tokio::task::spawn_blocking(move || changed_since(&root, &snapshot.stamps))
            .await
            .unwrap_or_default();
        if !files.is_empty() {
            tracing::info!(
                workspace = %workspace_str,
                files = files.len(),
                "🛠️ [EXEC] sending back files the command changed"
            );
            if let Err(e) = framed
                .send(WireMessage::ExecChanges(ExecChanges {
                    files: files.clone(),
                }))
                .await
            {
                // The client never receives these changes, so the copy must not keep them.
                let restored = restore_after_lost_client(
                    workspace_manager,
                    &workspace,
                    before,
                    snapshot_started,
                )
                .await;
                tracing::info!(
                    workspace = %workspace_str,
                    "🛠️ [EXEC] client left before the changes were sent; {restored} file(s) the command changed restored"
                );
                return Err(e.into());
            }
            refresh_engines(workspace_manager, &workspace, &files).await;
        }
    }
    framed
        .send(WireMessage::ExecExit(ExecExit {
            exit_code,
            duration_ms,
            server_workspace_root: workspace_str,
            timed_out,
            error: exec_err,
            usage,
            platform: Some(prod_code_protocol::platform()),
        }))
        .await?;
    Ok(())
}

pub async fn run_remote_exec(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: RemoteExecRequest,
) -> Result<()> {
    run_remote_exec_with_ram(
        storage_root,
        metrics,
        workspace_manager,
        framed,
        req,
        false,
        None,
    )
    .await
}

pub async fn run_remote_exec_with_ram(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: RemoteExecRequest,
    build_cache_ram: bool,
    build_cache_dir: Option<&std::path::Path>,
) -> Result<()> {
    use tokio::io::AsyncReadExt;

    let start = Instant::now();
    let workspace = workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let workspace_str = workspace.to_string_lossy().to_string();
    let fail = |error: String| RemoteExecResult {
        exit_code: None,
        duration_ms: 0,
        server_workspace_root: workspace_str.clone(),
        timed_out: false,
        error: Some(error),
        usage: None,
        platform: Some(prod_code_protocol::platform()),
        diagnostics: Vec::new(),
        tests_passed: 0,
        tests_failed: 0,
        tests_skipped: 0,
        test_failures: Vec::new(),
        benches: Vec::new(),
    };

    if !workspace.is_dir() {
        framed
            .send(WireMessage::RemoteExecResult(fail(format!(
                "workspace {workspace_str} is not synced to this gateway"
            ))))
            .await?;
        return Ok(());
    }
    workspace::touch_last_used(&workspace);

    let argv = req.to_argv();
    let Some((program, args)) = argv.split_first() else {
        framed
            .send(WireMessage::RemoteExecResult(fail("empty command".to_string())))
            .await?;
        return Ok(());
    };

    let timeout_secs = if req.timeout_secs == 0 {
        EXEC_DEFAULT_TIMEOUT_SECS
    } else {
        req.timeout_secs
    };
    if timeout_secs > MAX_REMOTE_EXEC_TIMEOUT_SECS {
        framed
            .send(WireMessage::RemoteExecResult(fail(format!(
                "timeout_secs ({timeout_secs}) exceeds maximum allowed ({MAX_REMOTE_EXEC_TIMEOUT_SECS})"
            ))))
            .await?;
        return Ok(());
    }
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let deadline = match tokio::time::Instant::now().checked_add(timeout) {
        Some(d) => d,
        None => {
            framed
                .send(WireMessage::RemoteExecResult(fail(
                    "timeout_secs overflowed deadline calculation".to_string(),
                )))
                .await?;
            return Ok(());
        }
    };

    if let Some((free, total)) = workspace::free_and_total_bytes(&workspace)
        .or_else(|| workspace::free_and_total_bytes(storage_root))
    {
        let free_gb = free as f64 / (1024.0 * 1024.0 * 1024.0);
        let used_pct = if total > 0 {
            (1.0 - (free as f64 / total as f64)) * 100.0
        } else {
            0.0
        };
        let hostname = get_hostname();
        if free_gb < 5.0 || used_pct > 95.0 {
            framed
                .send(WireMessage::RemoteExecResult(fail(format!(
                    "refused: node {hostname} has only {free_gb:.1} GB free on {} ({used_pct:.0}% full)",
                    workspace.display()
                ))))
                .await?;
            return Ok(());
        }
        if free_gb < 10.0 {
            let warn = format!(
                "[prod-code exec] WARNING: {free_gb:.1} GB free on node {hostname} ({used_pct:.0}% used)\n"
            );
            let _ = framed
                .send(WireMessage::RemoteExecStream(RemoteExecStream::Chunk(ExecChunk {
                    stderr: true,
                    data: Some(warn.into_bytes()),
                })))
                .await;
        }
    }

    let snapshot_started = Instant::now();
    let before = Arc::new(if req.pull_changes {
        let root = workspace.clone();
        tokio::task::spawn_blocking(move || snapshot_tree(&root))
            .await
            .unwrap_or_default()
    } else {
        TreeSnapshot::default()
    });

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

    let ram_target = resolve_ram_build_cache(&workspace, build_cache_ram, build_cache_dir);
    let _ram_lease = ram_target.as_deref().map(RamBuildLease::acquire);
    let (mut cmd, report) = exec_shim::command(program);
    cmd.args(args)
        .current_dir(&run_dir)
        .envs(polyglot_compiler_cache_env(&workspace, on_path("ccache"), ram_target.as_deref()))
        .envs(req.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    cmd.scrub_cluster_secrets();
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            framed
                .send(WireMessage::RemoteExecResult(fail(format!(
                    "failed to start {program}: {e}"
                ))))
                .await?;
            return Ok(());
        }
    };

    tracing::info!(
        workspace = %workspace_str,
        command = %argv.join(" "),
        language = ?req.language,
        "🛠️ [REMOTE_EXEC] started"
    );
    let _running = RunningEntry::start(&workspace, &argv);

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

    let mut timed_out = false;
    let mut status = None;
    let mut chunks_open = true;
    let mut client_left = false;

    let mut stdout_line_buf = String::new();
    let mut diagnostics = Vec::new();
    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut tests_skipped = 0;
    let mut test_failures = Vec::new();
    let mut benches = Vec::new();

    loop {
        tokio::select! {
            chunk = rx.recv(), if chunks_open => match chunk {
                Ok(chunk) => {
                    let is_stderr = chunk.stderr;
                    let chunk_data = chunk.data.clone();
                    if framed.send(WireMessage::RemoteExecStream(RemoteExecStream::Chunk(chunk))).await.is_err() {
                        client_left = true;
                    } else if (req.format == RemoteExecFormat::Json || matches!(req.command, RemoteExecCommand::Test | RemoteExecCommand::Bench)) && !is_stderr
                        && let Some(bytes) = chunk_data
                            && let Ok(text) = std::str::from_utf8(&bytes) {
                                stdout_line_buf.push_str(text);
                                while let Some(pos) = stdout_line_buf.find('\n') {
                                    if pos > MAX_JSON_LINE_BUFFER_BYTES {
                                        tracing::warn!(
                                            line_len = pos,
                                            "stdout line exceeded limit of {MAX_JSON_LINE_BUFFER_BYTES} bytes; dropping unparsed line"
                                        );
                                        stdout_line_buf.drain(..=pos);
                                        continue;
                                    }
                                    let line = stdout_line_buf[..pos].trim_end().to_string();
                                    stdout_line_buf.drain(..=pos);
                                    if line.is_empty() {
                                        continue;
                                    }
                                    let stream_event = match req.language {
                                        RemoteExecLanguage::Rust => parse_cargo_json_event(&line),
                                        RemoteExecLanguage::Go => parse_go_test_json_event(&line),
                                        _ => None,
                                    };
                                    if let Some(ev) = stream_event {
                                        match &ev {
                                            RemoteExecStream::Diagnostic(diag) => {
                                                diagnostics.push(diag.clone());
                                            }
                                            RemoteExecStream::TestEvent(test_ev) => match test_ev {
                                                RemoteExecTestEvent::Passed { .. } => tests_passed += 1,
                                                RemoteExecTestEvent::Failed { .. } => {
                                                    tests_failed += 1;
                                                    test_failures.push(test_ev.clone());
                                                }
                                                RemoteExecTestEvent::Skipped { .. } => tests_skipped += 1,
                                                RemoteExecTestEvent::Bench { .. } => benches.push(test_ev.clone()),
                                                _ => {}
                                            },
                                            _ => {}
                                        }
                                        if framed.send(WireMessage::RemoteExecStream(ev)).await.is_err() {
                                            client_left = true;
                                            break;
                                        }
                                    }
                                }
                                if stdout_line_buf.len() > MAX_JSON_LINE_BUFFER_BYTES {
                                    tracing::warn!(
                                        buf_len = stdout_line_buf.len(),
                                        "stdout buffer without newline exceeded limit of {MAX_JSON_LINE_BUFFER_BYTES} bytes; dropping unparsed buffer"
                                    );
                                    stdout_line_buf.clear();
                                }
                            }
                }
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
                Some(Ok(WireMessage::Ping)) => {
                    client_left = framed.send(WireMessage::Pong).await.is_err();
                }
                Some(Ok(WireMessage::Disconnect { .. })) | Some(Err(_)) | None => client_left = true,
                _ => {}
            }
        }
        if client_left || (!chunks_open && status.is_some()) {
            break;
        }
    }

    if client_left {
        kill_exec_group(pid, &exited);
        drop(rx);
        if status.is_none() {
            let _ = exit_rx.await;
        }
        if req.pull_changes {
            let restored =
                restore_after_lost_client(workspace_manager, &workspace, before, snapshot_started)
                    .await;
            tracing::info!(
                workspace = %workspace_str,
                "🛠️ [REMOTE_EXEC] client left; command killed; {restored} file(s) it changed restored"
            );
        } else {
            tracing::info!(workspace = %workspace_str, "🛠️ [REMOTE_EXEC] client left; command killed");
        }
        return Ok(());
    }

    for reader in readers {
        let _ = reader.await;
    }

    if (req.format == RemoteExecFormat::Json
        || matches!(req.command, RemoteExecCommand::Test | RemoteExecCommand::Bench))
        && !stdout_line_buf.is_empty()
    {
        let line = stdout_line_buf.trim_end().to_string();
        if !line.is_empty() && line.len() <= MAX_JSON_LINE_BUFFER_BYTES {
            let stream_event = match req.language {
                RemoteExecLanguage::Rust => parse_cargo_json_event(&line),
                RemoteExecLanguage::Go => parse_go_test_json_event(&line),
                _ => None,
            };
            if let Some(ev) = stream_event {
                match &ev {
                    RemoteExecStream::Diagnostic(diag) => diagnostics.push(diag.clone()),
                    RemoteExecStream::TestEvent(test_ev) => match test_ev {
                        RemoteExecTestEvent::Passed { .. } => tests_passed += 1,
                        RemoteExecTestEvent::Failed { .. } => {
                            tests_failed += 1;
                            test_failures.push(test_ev.clone());
                        }
                        RemoteExecTestEvent::Skipped { .. } => tests_skipped += 1,
                        RemoteExecTestEvent::Bench { .. } => benches.push(test_ev.clone()),
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }

    let report_status = report.as_ref().and_then(exec_shim::ReportFile::read);
    let shim_raw_status = status.flatten();
    let had_report = report.is_some();
    drop(report);
    let (exit_code, usage, exec_err) = if let Some((raw, usage)) = report_status {
        use std::os::unix::process::ExitStatusExt;
        (std::process::ExitStatus::from_raw(raw).code(), Some(usage), None)
    } else if let Some((raw, usage)) = shim_raw_status {
        use std::os::unix::process::ExitStatusExt;
        let exit_status = std::process::ExitStatus::from_raw(raw);
        if exit_status.code() == Some(74) && had_report && !timed_out {
            (
                Some(74),
                Some(usage),
                Some("exec shim failed to write process report (disk full or write error)".to_string()),
            )
        } else {
            (exit_status.code(), Some(usage), None)
        }
    } else {
        (None, None, None)
    };

    if exit_code == Some(254) {
        tokio::task::spawn_blocking(crate::shadow::ensure_sccache_server).await.ok();
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    tracing::info!(
        workspace = %workspace_str,
        command = %argv.join(" "),
        exit_code = ?exit_code,
        timed_out,
        duration_ms,
        "🛠️ [REMOTE_EXEC] finished"
    );

    {
        let mut ev = metrics::Event::blank("remote_exec");
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
        ev.command = argv.join(" ");
        ev.duration_ms = duration_ms;
        ev.exit_code = exit_code;
        ev.ok = exit_code == Some(0) && exec_err.is_none();
        metrics.record(ev);
    }

    if req.pull_changes {
        let (root, snapshot) = (workspace.clone(), Arc::clone(&before));
        let files = tokio::task::spawn_blocking(move || changed_since(&root, &snapshot.stamps))
            .await
            .unwrap_or_default();
        if !files.is_empty() {
            if let Err(e) = framed
                .send(WireMessage::ExecChanges(ExecChanges {
                    files: files.clone(),
                }))
                .await
            {
                let restored = restore_after_lost_client(
                    workspace_manager,
                    &workspace,
                    before,
                    snapshot_started,
                )
                .await;
                tracing::info!(
                    workspace = %workspace_str,
                    "🛠️ [REMOTE_EXEC] client left before the changes were sent; {restored} file(s) restored"
                );
                return Err(e.into());
            }
            refresh_engines(workspace_manager, &workspace, &files).await;
        }
    }

    framed
        .send(WireMessage::RemoteExecResult(RemoteExecResult {
            exit_code,
            duration_ms,
            server_workspace_root: workspace_str,
            timed_out,
            error: exec_err,
            usage,
            platform: Some(prod_code_protocol::platform()),
            diagnostics,
            tests_passed,
            tests_failed,
            tests_skipped,
            test_failures,
            benches,
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
    exec_shim::reap_with_usage(pid)
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

    let arrived: Vec<String> = req.files.iter().map(|f| f.relative_path.clone()).collect();
    let synced: Vec<(String, Option<u64>)> = req
        .files
        .iter()
        .map(|f| {
            (
                f.relative_path.clone(),
                f.content.as_deref().map(content_hash),
            )
        })
        .collect();
    let mut project_config_changed = false;
    let mut watched = Vec::new();
    // Files that could not be written: not recorded as synced, and reported stale so that the
    // client sends them again (#385).
    let mut failed: Vec<String> = Vec::new();
    for delta in req.files {
        let target_path = match safe_sync_target(&server_workspace, &delta.relative_path).await {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(error = %error, file = %delta.relative_path, "sync rejected unsafe path");
                failed.push(delta.relative_path);
                continue;
            }
        };
        project_config_changed |= is_project_config_file(&delta.relative_path);
        match delta.content {
            Some(content_bytes) => {
                bytes_transferred += content_bytes.len();
                let kind = if target_path.exists() {
                    workspace::WatchedChange::Changed
                } else {
                    workspace::WatchedChange::Created
                };
                if let Err(e) =
                    write_synced_file(&target_path, &content_bytes, delta.is_executable).await
                {
                    tracing::warn!(error = %e, file = %target_path.display(), "sync write failed; the client sends it again");
                    failed.push(delta.relative_path);
                    continue;
                }
                files_updated += 1;
                watched.push((target_path.clone(), kind));
                if let Ok(text) = std::str::from_utf8(&content_bytes) {
                    for engine_lock in &loaded_rust {
                        let mut engine = engine_lock.lock().await;
                        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            engine.update_base(&target_path, Some(text.to_string()))
                        }));
                        match res {
                            Ok(Err(e)) => {
                                tracing::warn!(error = %e, file = %target_path.display(), "base update failed");
                            }
                            Err(_) => {
                                tracing::warn!(file = %target_path.display(), "base update panicked; continuing");
                            }
                            Ok(Ok(())) => {}
                        }
                    }
                }
            }
            None => {
                if target_path.exists() && tokio::fs::remove_file(&target_path).await.is_ok() {
                    files_deleted += 1;
                    watched.push((target_path.clone(), workspace::WatchedChange::Deleted));
                    prune_empty_parents(&server_workspace, target_path.parent());
                }
                for engine_lock in &loaded_rust {
                    let mut engine = engine_lock.lock().await;
                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        engine.update_base(&target_path, None)
                    }));
                    match res {
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, file = %target_path.display(), "base removal failed");
                        }
                        Err(_) => {
                            tracing::warn!(file = %target_path.display(), "base removal panicked; continuing");
                        }
                        Ok(Ok(())) => {}
                    }
                }
            }
        }
    }
    let synced: Vec<(String, Option<u64>)> = synced
        .into_iter()
        .filter(|(path, _)| !failed.contains(path))
        .collect();
    workspace::record_synced(&server_workspace, &synced);
    workspace::touch_last_used(&server_workspace);
    let mut stale_paths =
        workspace::clear_stale_paths(&server_workspace, arrived.iter().map(String::as_str));
    workspace::record_stale_paths(&server_workspace, &failed);
    for path in failed {
        if !stale_paths.contains(&path) {
            stale_paths.push(path);
        }
    }
    // gopls does not watch the tree itself: without this it went on answering from the content
    // a file no session had open had when it first read it (#317).
    for loaded in workspace_manager.loaded_under(&server_workspace).await {
        loaded.notify_watched_files(&watched).await;
    }
    workspace_manager.editor_servers.notify(&watched).await;

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

    if files_updated > 0 || files_deleted > 0 || workspace_was_fresh {
        tracing::info!(
            folder_name,
            files_updated,
            files_deleted,
            bytes_transferred,
            fresh = workspace_was_fresh,
            duration_ms = %format!("{duration_ms}ms"),
            "[SYNC] Workspace fast-sync applied"
        );
    } else {
        tracing::debug!(
            folder_name,
            duration_ms = %format!("{duration_ms}ms"),
            "[SYNC] Workspace fast-sync (no changes)"
        );
    }

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
        stale_paths,
    }
}

pub async fn handle_client(
    stream: impl Into<AnyStream>,
    addr: impl std::fmt::Display,
    state: Arc<ServerState>,
) -> Result<()> {
    let mut framed = Framed::new(stream.into(), ProdCodeCodec::new());

    // A gateway with a token serves nothing, not even its status, to a connection that does
    // not open with it (#402).
    if let Some(expected) = state.auth_token.as_deref() {
        let first = tokio::time::timeout(AUTH_WAIT, framed.next())
            .await
            .ok()
            .flatten();
        let presented = match &first {
            Some(Ok(WireMessage::Auth(token))) => Some(token),
            _ => None,
        };
        if !presented.is_some_and(|token| token.matches(expected)) {
            tracing::warn!(
                %addr,
                presented = presented.is_some(),
                "🔒 [AUTH] closed a connection without the cluster's token"
            );
            let _ = framed
                .send(WireMessage::Disconnect {
                    reason: AUTH_REFUSED.to_string(),
                })
                .await;
            return Ok(());
        }
    }

    while let Some(msg_res) = framed.next().await {
        let msg = msg_res?;
        match msg {
            // A token sent to a gateway that requires none, or sent twice, changes nothing.
            WireMessage::Auth(_) => {}
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
                    let admission = Arc::clone(state.workspace_manager.admission());
                    tokio::spawn(async move {
                        if let Ok(view) = loaded.validation_view(&admission).await
                            && let Some(engine) = view.rust_engine.clone()
                        {
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
                run_exec_with_ram(
                    &state.storage_root,
                    &state.metrics,
                    &state.workspace_manager,
                    &mut framed,
                    req,
                    state.build_cache_ram,
                    state.build_cache_dir.as_deref(),
                )
                .await?;
            }
            WireMessage::RemoteExecRequest(req) => {
                run_remote_exec_with_ram(
                    &state.storage_root,
                    &state.metrics,
                    &state.workspace_manager,
                    &mut framed,
                    req,
                    state.build_cache_ram,
                    state.build_cache_dir.as_deref(),
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
                let protocol_version = match negotiate_protocol_version(&req) {
                    Ok(version) => version,
                    Err(err) => {
                        let reason = format!("gateway refused protocol negotiation: {err}");
                        tracing::warn!(reason, "refusing incompatible handshake");
                        framed.send(WireMessage::Disconnect { reason }).await?;
                        return Ok(());
                    }
                };
                let session_id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
                let _active_session = ActiveSession::start(&state.active_sessions);
                let session_capabilities = prod_code_protocol::negotiate_capabilities(
                    req.capabilities.as_ref(),
                    &prod_code_protocol::default_server_capabilities(),
                );

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
                    if req.redirect_count < 2 {
                        let view = state.cluster_view().await;
                        if let Some(target) = view.nodes.iter().find(|n| {
                            n.alive && cluster_supports_engine(&n.status, engine)
                        }) {
                            tracing::info!(
                                engine,
                                target = %target.addr,
                                "redirecting client to cluster node serving engine"
                            );
                            let _ = framed
                                .send(WireMessage::Redirect {
                                    target_addr: target.addr.clone(),
                                    reason: Some(format!("engine {engine} is served by {}", target.addr)),
                                })
                                .await;
                            return Ok(());
                        }
                    }
                    let reason = format!(
                        "engine {engine} is not served by this node (--engines {}); pick a node that lists it",
                        state.engine_allowlist.join(",")
                    );
                    tracing::warn!(
                        client_root = %req.client_workspace_root,
                        engine,
                        "refusing handshake: engine not served here"
                    );
                    framed.send(WireMessage::Disconnect { reason }).await?;
                    return Ok(());
                }

                // Roadmap 5.1: If this workspace is not already loaded locally, but another live cluster
                // node has it loaded warm, transparently redirect the client there.
                let is_loaded_locally = state
                    .workspace_manager
                    .get_loaded(&server_workspace)
                    .await
                    .is_some();
                if req.redirect_count < 3 {
                    let view = state.cluster_view().await;
                    let own_addr = state.advertise.read().await.clone();
                    let ws_name = req
                        .base_workspace_name
                        .as_deref()
                        .unwrap_or(&req.client_workspace_root);
                    if !is_loaded_locally && req.redirect_count == 0 {
                        if let Some(holder) = view.nodes.iter().find(|n| {
                            n.alive
                                && n.addr != own_addr
                                && cluster_supports_engine(&n.status, engine)
                                && n.workspaces.iter().any(|w| w.name == ws_name)
                        }) {
                            tracing::info!(
                                workspace = ws_name,
                                target = %holder.addr,
                                "transparently redirecting client to node with warm workspace engine"
                            );
                            let _ = framed
                                .send(WireMessage::Redirect {
                                    target_addr: holder.addr.clone(),
                                    reason: Some(format!(
                                        "workspace {ws_name} is already warm on {}",
                                        holder.addr
                                    )),
                                })
                                .await;
                            return Ok(());
                        }
                    }

                    // Roadmap 5.3: Dynamic workload rebalancing and resource pressure load shedding.
                    // If this gateway is under resource pressure or congested, and another live node is roomy,
                    // redirect this workspace connection to the quietest roomiest node!
                    // If already loaded locally, require severe congestion or hard pressure to avoid unnecessary churn.
                    let current_status = state.status().await;
                    let under_pressure = current_status.host.pressure().is_some();
                    let congested = if is_loaded_locally {
                        under_pressure || current_status.congestion_score() >= 1.5
                    } else {
                        under_pressure || current_status.congestion_score() >= 1.2
                    };
                    if congested {
                        let own_score = current_status.congestion_score();
                        if let Some(roomy) = view
                            .nodes
                            .iter()
                            .filter(|n| {
                                n.alive
                                    && n.addr != own_addr
                                    && cluster_supports_engine(&n.status, engine)
                                    && n.status.host.pressure().is_none()
                                    && n.status.congestion_score() < own_score * 0.6
                            })
                            .min_by(|a, b| {
                                a.status
                                    .congestion_score()
                                    .partial_cmp(&b.status.congestion_score())
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                        {
                            tracing::info!(
                                workspace = ws_name,
                                target = %roomy.addr,
                                loaded = is_loaded_locally,
                                "transparently redirecting client from congested gateway to roomier node"
                            );
                            let _ = framed
                                .send(WireMessage::Redirect {
                                    target_addr: roomy.addr.clone(),
                                    reason: Some(format!(
                                        "node {own_addr} is congested (score {:.2}); redirected to roomier node {}",
                                        own_score,
                                        roomy.addr
                                    )),
                                })
                                .await;
                            return Ok(());
                        }
                    }
                }

                let translator =
                    PathTranslator::new(&req.client_workspace_root, &server_workspace_str);

                // An editor gets the language server it would run locally, a process of its own
                // on this node (#332); without one here, the shared engines answer it.
                if req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_EDITOR)
                    && editor_proxy::enabled()
                    && let Some(command) = editor_proxy::server_command(engine)
                {
                    framed
                        .send(WireMessage::HandshakeResponse(HandshakeResponse {
                            protocol_version,
                            server_pid: state.server_pid,
                            session_id,
                            server_workspace_root: server_workspace_str.clone(),
                            detected_engine: engine.to_string(),
                            stale_paths: workspace::stale_paths(&server_workspace),
                            engine_age_ms: None,
                            index_gated: false,
                            capabilities: Some(session_capabilities.clone()),
                        }))
                        .await?;
                    let outcome = editor_proxy::run(
                        framed,
                        translator,
                        command,
                        &engine_root,
                        &state.workspace_manager.editor_servers,
                        session_id,
                    )
                    .await;
                    return outcome;
                }

                // Attach to shared workspace using leader-follower coalescing. A load refused
                // for capacity (#433), or failed, is told to the client, which says why.
                let shared_ws = match state
                    .workspace_manager
                    .get_or_load(&engine_root, engine)
                    .await
                {
                    Ok(shared_ws) => shared_ws,
                    Err(err) => {
                        let reason = format!("{err:#}");
                        tracing::warn!(
                            client_root = %req.client_workspace_root,
                            engine,
                            reason,
                            "refusing handshake: the engine could not be loaded"
                        );
                        framed.send(WireMessage::Disconnect { reason }).await?;
                        return Ok(());
                    }
                };
                let engine_age_ms = shared_ws.loaded_at.elapsed().as_millis() as u64;
                // The in-process Rust engine answers from a complete analysis once loaded; gopls
                // and the servers whose readiness is known are waited for (#391).
                let index_gated = shared_ws.rust_engine.is_some()
                    || shared_ws.go_engine.is_some()
                    || shared_ws
                        .generic_engine
                        .as_ref()
                        .is_some_and(|engine| engine.readiness_known());

                let validation =
                    req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_VALIDATION);
                let generic_validation_session = if validation && shared_ws.generic_engine.is_some()
                {
                    Some(Arc::clone(&shared_ws.generic_validation_session))
                } else {
                    None
                };
                let mut session_view = state
                    .workspace_manager
                    .register_session_view(session_id, client_root_path.clone(), shared_ws)
                    .await;
                // A session that only validates proposed texts runs on the workspace's second
                // engine, so its overlays never invalidate the main one (#73).
                let _generic_validation_session = if let Some(serial) = generic_validation_session {
                    Some(serial.lock_owned().await)
                } else {
                    None
                };
                if validation {
                    let validation_view = session_view
                        .accounted
                        .validation_view(state.workspace_manager.admission())
                        .await;
                    match validation_view {
                        Ok(view) => session_view.workspace = view,
                        Err(err) => {
                            let is_capacity = err
                                .downcast_ref::<crate::admission::CapacityRefused>()
                                .is_some()
                                || err.root_cause().is::<crate::admission::CapacityRefused>()
                                || err.to_string().contains("capacity:");

                            if is_capacity && req.redirect_count < 2 {
                                let view = state.cluster_view().await;
                                let own_addr = state.advertise.read().await.clone();
                                let ws_name = req
                                    .base_workspace_name
                                    .as_deref()
                                    .unwrap_or(&req.client_workspace_root);
                                let target = view
                                    .nodes
                                    .iter()
                                    .filter(|n| {
                                        n.alive
                                            && !n.addr.is_empty()
                                            && n.addr != own_addr
                                            && n.addr != view.this_node
                                            && cluster_supports_engine(&n.status, engine)
                                            && n.status.host.pressure().is_none()
                                    })
                                    .max_by_key(|n| n.workspaces.iter().any(|w| w.name == ws_name));

                                if let Some(target) = target {
                                    tracing::info!(
                                        session_id,
                                        engine,
                                        target = %target.addr,
                                        "redirecting validation session under gateway memory pressure"
                                    );
                                    state
                                        .workspace_manager
                                        .unregister_session_view(session_view)
                                        .await;
                                    let _ = framed
                                        .send(WireMessage::Redirect {
                                            target_addr: target.addr.clone(),
                                            reason: Some(format!(
                                                "gateway memory pressure; validating {engine} on {}",
                                                target.addr
                                            )),
                                        })
                                        .await;
                                    return Ok(());
                                }
                            }

                            if is_capacity
                                && engine != "cpp"
                                && session_view.accounted.generic_engine.is_some()
                            {
                                tracing::warn!(
                                    session_id,
                                    engine,
                                    "private validation engine unavailable under memory pressure; validating on the main engine"
                                );
                            } else {
                                let reason = format!("private validation engine unavailable: {err:#}");
                                tracing::warn!(
                                    session_id,
                                    engine,
                                    reason,
                                    "refusing validation handshake"
                                );
                                state
                                    .workspace_manager
                                    .unregister_session_view(session_view)
                                    .await;
                                framed.send(WireMessage::Disconnect { reason }).await?;
                                return Ok(());
                            }
                        }
                    }
                }

                tracing::info!(
                    session_id,
                    client_pid = req.client_pid,
                    client_root = %req.client_workspace_root,
                    server_root = %server_workspace_str,
                    engine_root = %engine_root.display(),
                    engine,
                    is_single_owner = session_view.is_single_owner(),
                    "Client session established (Direct-Edit fast path active: {})",
                    session_view.is_single_owner()
                );

                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version,
                        server_pid: state.server_pid,
                        session_id,
                        server_workspace_root: server_workspace_str,
                        detected_engine: engine.to_string(),
                        stale_paths: workspace::stale_paths(&server_workspace),
                        engine_age_ms: Some(engine_age_ms),
                        index_gated,
                        capabilities: Some(session_capabilities),
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
                    storage_root: state.storage_root.clone(),
                    metrics: Arc::clone(&state.metrics),
                    editor: req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_EDITOR),
                    edits: Arc::default(),
                });
                let session_res = run_session_loop(framed, &translator, &session_view, meta).await;

                state
                    .workspace_manager
                    .unregister_session_view(session_view)
                    .await;

                tracing::debug!(session_id, "Client session retired: {:?}", session_res);
                return session_res;
            }
            WireMessage::Disconnect { reason } => {
                tracing::debug!(%addr, reason, "Client disconnected cleanly");
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
    framed: Framed<AnyStream, ProdCodeCodec>,
    translator: &PathTranslator,
    view: &SessionView,
    meta: Arc<SessionMeta>,
) -> Result<()> {
    let (mut socket_tx, mut socket_rx) = framed.split();
    // rapidfire MPSC: every engine task sends, one writer drains in batches and flushes the
    // socket once per batch.
    let (raw_out_tx, mut out_rx) =
        rapidfire::mpsc::bounded::<SharedOutputFrame>(SHARED_OUTPUT_CAPACITY);
    let out_tx = SharedOutputSender::new(raw_out_tx, SHARED_OUTPUT_WRITE_BUDGET);

    // Requests in flight, keyed by JSON-RPC id, so every answer — whichever engine produced
    // it — becomes one metrics event with its duration.
    let pending: Arc<tokio::sync::Mutex<std::collections::HashMap<String, PendingRequest>>> =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let pending_writer = Arc::clone(&pending);
    let meta_writer = Arc::clone(&meta);
    let writer_output = out_tx.clone();
    let writer_handle = tokio::spawn(async move {
        let _lifetime = SharedWriterLifetime::start(writer_output);
        let mut batch: Vec<SharedOutputFrame> = Vec::with_capacity(SHARED_OUTPUT_BATCH);
        while out_rx
            .recv_many(&mut batch, SHARED_OUTPUT_BATCH)
            .await
            .is_ok()
        {
            let mut flush_deadline = None;
            for frame in batch.drain(..) {
                if tokio::time::Instant::now() >= frame.deadline {
                    anyhow::bail!("shared output frame expired while queued");
                }
                if let WireMessage::LspPayload(ref raw) = frame.message
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
                tokio::time::timeout_at(frame.deadline, socket_tx.feed(frame.message))
                    .await
                    .map_err(|_| anyhow::anyhow!("shared socket feed deadline elapsed"))??;
                flush_deadline = Some(match flush_deadline {
                    Some(current) => std::cmp::min(current, frame.deadline),
                    None => frame.deadline,
                });
            }
            if let Some(deadline) = flush_deadline {
                tokio::time::timeout_at(deadline, socket_tx.flush())
                    .await
                    .map_err(|_| anyhow::anyhow!("shared socket flush deadline elapsed"))??;
            }
        }
        Ok(())
    });
    // Install abort ownership before the session can reach another await. Cancellation of the
    // handler must cancel this exact writer, whose lifetime guard closes every producer queue.
    let mut writer = OwnedJoin::new(writer_handle);

    // gopls and the supervised servers answer their own requests (`window/workDoneProgress/create`,
    // `workspace/configuration`) in the engine; passed on, one carried the id of a client's
    // question and was taken for its answer (#391). Only their notifications go to the client.
    let engine_answers_requests =
        view.workspace.go_engine.is_some() || view.workspace.generic_engine.is_some();
    let mut backend_rx = if let Some(ref go) = view.workspace.go_engine {
        Some(go.subscribe())
    } else if let Some(ref generic_eng) = view.workspace.generic_engine {
        Some(generic_eng.subscribe())
    } else {
        view.workspace.backend.as_ref().map(|b| b.subscribe())
    };

    let mut rebalance_rx = view.accounted.subscribe_rebalance();
    let mut writer_finished = false;
    let mut session_result = Ok(());
    loop {
        tokio::select! {
            writer_result = writer.task_mut() => {
                writer.clear_finished();
                writer_finished = true;
                session_result = flatten_writer_result(writer_result);
                break;
            }
            client_msg_res = socket_rx.next() => {
                match on_client_message(client_msg_res, &out_tx, translator, view, &meta, &pending).await {
                    Flow::Next => continue,
                    Flow::Stop => break,
                }
            }

            rebalance_msg = rebalance_rx.recv() => {
                match rebalance_msg {
                    Ok((target_addr, reason)) => {
                        if meta.editor {
                            continue;
                        }
                        tracing::info!(
                            session_id = meta.session_id,
                            target = %target_addr,
                            ?reason,
                            "Session rebalanced: sending Redirect frame to active client"
                        );
                        let _ = out_tx.send(WireMessage::Redirect { target_addr, reason }).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
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
                        if (engine_answers_requests && is_server_request(&server_lsp))
                            || (!engine_answers_requests && fallback_answers_request(&server_lsp))
                        {
                            continue;
                        }
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
    out_tx.close();
    if !writer_finished {
        let teardown_deadline = tokio::time::Instant::now() + SHARED_OUTPUT_TEARDOWN_BUDGET;
        match tokio::time::timeout_at(teardown_deadline, writer.task_mut()).await {
            Ok(writer_result) => {
                writer.clear_finished();
                let writer_result = flatten_writer_result(writer_result);
                if session_result.is_ok() {
                    session_result = writer_result;
                }
            }
            Err(_) => {
                writer.abort();
                // Once aborted, await the exact task so no writer is detached. This is cleanup
                // after the single teardown deadline, not a second drain budget.
                let writer_result = writer.task_mut().await;
                writer.clear_finished();
                if let Err(error) = writer_result
                    && !error.is_cancelled()
                {
                    tracing::warn!(%error, "shared output writer failed while being aborted");
                }
                if session_result.is_ok() {
                    session_result = Err(anyhow::anyhow!(
                        "shared output writer exceeded teardown budget"
                    ));
                }
            }
        }
    }
    session_result
}

const SHARED_OUTPUT_CAPACITY: usize = 64;
const SHARED_OUTPUT_BATCH: usize = 64;
const SHARED_OUTPUT_WRITE_BUDGET: Duration = Duration::from_secs(2);
const SHARED_OUTPUT_TEARDOWN_BUDGET: Duration = Duration::from_secs(3);

#[doc(hidden)]
pub static ACTIVE_SHARED_OUTPUT_WRITERS: AtomicUsize = AtomicUsize::new(0);

struct SharedOutputFrame {
    message: WireMessage,
    deadline: tokio::time::Instant,
}

#[derive(Clone)]
struct SharedOutputSender {
    inner: rapidfire::mpsc::Sender<SharedOutputFrame>,
    write_budget: Duration,
}

#[derive(Debug)]
enum SharedOutputSendError {
    Closed,
    Deadline,
}

impl SharedOutputSender {
    fn new(inner: rapidfire::mpsc::Sender<SharedOutputFrame>, write_budget: Duration) -> Self {
        Self {
            inner,
            write_budget,
        }
    }

    async fn send(&self, message: WireMessage) -> std::result::Result<(), SharedOutputSendError> {
        let deadline = tokio::time::Instant::now() + self.write_budget;
        let frame = SharedOutputFrame { message, deadline };
        match tokio::time::timeout_at(deadline, self.inner.send(frame)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(SharedOutputSendError::Closed),
            Err(_) => {
                // One expired producer expires the generation. This wakes every queue waiter and
                // prevents later notifications from repeatedly extending a dead client's life.
                self.close();
                Err(SharedOutputSendError::Deadline)
            }
        }
    }

    fn close(&self) {
        self.inner.close();
    }
}

struct SharedWriterLifetime {
    output: SharedOutputSender,
}

impl SharedWriterLifetime {
    fn start(output: SharedOutputSender) -> Self {
        ACTIVE_SHARED_OUTPUT_WRITERS.fetch_add(1, Ordering::Relaxed);
        Self { output }
    }
}

impl Drop for SharedWriterLifetime {
    fn drop(&mut self) {
        self.output.close();
        ACTIVE_SHARED_OUTPUT_WRITERS.fetch_sub(1, Ordering::Relaxed);
    }
}

struct OwnedJoin<T> {
    task: Option<tokio::task::JoinHandle<T>>,
}

impl<T> OwnedJoin<T> {
    fn new(task: tokio::task::JoinHandle<T>) -> Self {
        Self { task: Some(task) }
    }

    fn task_mut(&mut self) -> &mut tokio::task::JoinHandle<T> {
        self.task.as_mut().expect("owned task is live")
    }

    fn abort(&self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }

    fn clear_finished(&mut self) {
        let task = self.task.take().expect("owned task is live");
        debug_assert!(task.is_finished());
    }
}

impl<T> Drop for OwnedJoin<T> {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn flatten_writer_result(
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    result.map_err(|error| anyhow::anyhow!("shared output writer task failed: {error}"))?
}

/// The capabilities an editor's `initialize` is answered with (#310).
///
/// A language server on the node advertises its own, except for document sync: the gateway
/// hands every change on as the document's full text, so the editor is asked for exactly that
/// whatever the server would take. The in-memory Rust engine advertises what it answers, with
/// the trigger characters rust-analyzer's own server uses; a workspace with neither only takes
/// documents.
fn editor_capabilities(server: Option<serde_json::Value>, rust: bool) -> serde_json::Value {
    let mut caps = match server {
        Some(caps) if caps.is_object() => caps,
        _ if rust => serde_json::json!({
            "hoverProvider": true,
            "definitionProvider": true,
            "referencesProvider": true,
            "implementationProvider": true,
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
            "renameProvider": true,
            "callHierarchyProvider": true,
            "completionProvider": {
                "triggerCharacters": [":", ".", "'", "("],
                "resolveProvider": true
            },
            "signatureHelpProvider": { "triggerCharacters": ["(", ",", "<"] },
            "inlayHintProvider": true,
            "documentHighlightProvider": true,
            "codeActionProvider": { "resolveProvider": true },
            "documentFormattingProvider": true
        }),
        _ => serde_json::json!({}),
    };
    let save = caps.pointer("/textDocumentSync/save").cloned();
    caps["textDocumentSync"] = serde_json::json!({ "openClose": true, "change": 1 });
    if let Some(save) = save {
        caps["textDocumentSync"]["save"] = save;
    }
    caps
}

/// What the session loop does once a client message has been handled.
enum Flow {
    /// Wait for the next message.
    Next,
    /// The client is gone or asked to disconnect: end the session.
    Stop,
}

/// Decode an LSP's zero-based position into the one-based coordinates used by the Rust engine.
///
/// LSP positions must be non-negative JSON integers. The engine's one-based API also means
/// that `u32::MAX` cannot be represented, so reject it rather than truncating or overflowing.
fn one_based_position(position: Option<&serde_json::Value>) -> Result<(u32, u32), &'static str> {
    let position = position.ok_or("position is required")?;
    let coordinate = |name| {
        position
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .and_then(|value| value.checked_add(1))
            .ok_or("position coordinates must be non-negative integers below 4294967295")
    };
    Ok((coordinate("line")?, coordinate("character")?))
}

/// Keep request metrics bounded even when a forwarded request has an arbitrary JSON shape.
fn metric_position(position: Option<&serde_json::Value>) -> (u32, u32) {
    one_based_position(position).unwrap_or((1, 1))
}

/// Validate only the Rust methods that consume LSP positions locally.
fn native_position_params(
    method: Option<&str>,
    params: Option<&serde_json::Value>,
) -> Result<(), String> {
    let params = params.unwrap_or(&serde_json::Value::Null);
    match method {
        Some(
            "textDocument/hover"
            | "textDocument/definition"
            | "textDocument/references"
            | "textDocument/implementation"
            | "textDocument/prepareCallHierarchy"
            | "prodCode/safeDelete"
            | "textDocument/rename",
        ) => one_based_position(params.get("position"))
            .map(|_| ())
            .map_err(str::to_owned),
        Some("callHierarchy/incomingCalls" | "callHierarchy/outgoingCalls") => one_based_position(
            params
                .get("item")
                .and_then(|item| item.get("selectionRange"))
                .and_then(|range| range.get("start")),
        )
        .map(|_| ())
        .map_err(|reason| format!("item.selectionRange.start: {reason}")),
        Some("prodCode/structuralReplace") => params
            .get("position")
            .map(|position| {
                one_based_position(Some(position))
                    .map(|_| ())
                    .map_err(str::to_owned)
            })
            .unwrap_or(Ok(())),
        Some("prodCode/assists" | "prodCode/applyAssist") => {
            one_based_position(params.pointer("/range/start"))
                .map_err(|reason| format!("range.start: {reason}"))?;
            if let Some(end) = params.pointer("/range/end") {
                one_based_position(Some(end)).map_err(|reason| format!("range.end: {reason}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn send_invalid_params(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    id: &serde_json::Value,
    method: &str,
    reason: &str,
) {
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32602,
            "message": format!("invalid params for {method}: {reason}"),
        }
    });
    let client_response = translator.translate_lsp_to_client(&response.to_string());
    let _ = out_tx.send(WireMessage::LspPayload(client_response)).await;
}

/// One message from the client: an LSP payload answered by the in-memory engine or forwarded
/// to the backend, a sync, a status request or a disconnect.
///
/// It lived inside the session loop's `tokio::select!`, where it was 1,400 lines of macro
/// input: rust-analyzer offers no refactoring inside a macro call, and every validation of the
/// file inferred it as one body (#86). Out here it is ordinary code.
async fn on_client_message(
    client_msg_res: Option<std::result::Result<WireMessage, std::io::Error>>,
    out_tx: &SharedOutputSender,
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
                single_owner = view.is_single_owner(),
                "Processing incoming LSP message"
            );

            // Inspect LSP message structure
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&server_lsp) {
                let method = val.get("method").and_then(|m| m.as_str());
                let id = val.get("id").cloned();
                if view.workspace.rust_engine.is_some()
                    && let Err(reason) = native_position_params(method, val.get("params"))
                {
                    if let (Some(method), Some(id)) =
                        (method, id.as_ref().filter(|id| !id.is_null()))
                    {
                        send_invalid_params(out_tx, translator, id, method, &reason).await;
                    }
                    return Flow::Next;
                }
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
                    let path = uri_or_path(uri);
                    let file = path
                        .strip_prefix(&meta.engine_root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned();
                    let pos = params.and_then(|p| {
                        p.get("position")
                            .or_else(|| p.get("range").and_then(|r| r.get("start")))
                            .or_else(|| {
                                p.get("item")
                                    .and_then(|item| item.get("selectionRange"))
                                    .and_then(|range| range.get("start"))
                            })
                    });
                    let (line, col) = metric_position(pos);
                    pending.lock().await.insert(
                        id_val.to_string(),
                        PendingRequest {
                            method: m.to_string(),
                            file,
                            line,
                            col,
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
                    let caps = editor_capabilities(caps, view.workspace.rust_engine.is_some());
                    let init_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": {
                            "capabilities": caps,
                            "serverInfo": {
                                "name": "prod-code",
                                "version": env!("CARGO_PKG_VERSION")
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
                        Some(m) if prod_code_engine_rust::editor::EDITOR_METHODS.contains(&m) => {
                            let params = val.get("params").cloned().unwrap_or_default();
                            lsp_editor_request(
                                out_tx,
                                translator,
                                view,
                                &id,
                                m,
                                params,
                                engine_lock,
                            );
                            return Flow::Next;
                        }
                        Some("textDocument/didOpen") => {
                            if let Some(params) = val.get("params") {
                                let uri = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let file_path = uri_or_path(uri);
                                if let Some(text) = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("text"))
                                    .and_then(|t| t.as_str())
                                {
                                    let edit_start = Instant::now();
                                    let text_len = text.len();
                                    {
                                        let mut engine = engine_lock.lock().await;
                                        if view.is_single_owner() {
                                            if let Err(e) = engine.apply_file_change(&file_path, text.to_string()) {
                                                tracing::warn!(error = %e, file = %file_path.display(), "direct-edit didOpen file change failed");
                                            }
                                            if let Ok(mut files) = view.direct_edit_open_files.lock() {
                                                files.insert(file_path.clone(), text.to_string());
                                            }
                                        } else {
                                            if let Err(e) = engine.set_session_overlay(
                                                view.session_id,
                                                &file_path,
                                                Some(text.to_string()),
                                            ) {
                                                tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                            }
                                        }
                                    }
                                    let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                    tracing::info!(
                                        session = view.session_id,
                                        file = %file_path.display(),
                                        bytes = text_len,
                                        duration_ms = format!("{:.2}ms", ms),
                                        single_owner = view.is_single_owner(),
                                        "📝 [EDIT] didOpen recorded in Salsa DB"
                                    );
                                    publish_rust_diagnostics(
                                        out_tx,
                                        translator,
                                        view,
                                        meta,
                                        file_path.clone(),
                                        engine_lock,
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
                                let file_path = uri_or_path(uri);
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
                                        if view.is_single_owner() {
                                            if let Err(e) = engine.apply_file_change(&file_path, text.to_string()) {
                                                tracing::warn!(error = %e, file = %file_path.display(), "direct-edit didChange file change failed");
                                            }
                                            if let Ok(mut files) = view.direct_edit_open_files.lock() {
                                                files.insert(file_path.clone(), text.to_string());
                                            }
                                        } else {
                                            if let Err(e) = engine.set_session_overlay(
                                                view.session_id,
                                                &file_path,
                                                Some(text.to_string()),
                                            ) {
                                                tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                            }
                                        }
                                    }
                                    let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                    tracing::info!(
                                        session = view.session_id,
                                        file = %file_path.display(),
                                        bytes = text_len,
                                        duration_ms = format!("{:.2}ms", ms),
                                        single_owner = view.is_single_owner(),
                                        "📝 [EDIT] didChange recorded in Salsa DB"
                                    );
                                    publish_rust_diagnostics(
                                        out_tx,
                                        translator,
                                        view,
                                        meta,
                                        file_path.clone(),
                                        engine_lock,
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
                                let file_path = uri_or_path(uri);
                                let mut engine = engine_lock.lock().await;
                                if view.is_single_owner() {
                                    if let Err(e) = engine.reload_file(&file_path) {
                                        tracing::warn!(error = %e, file = %file_path.display(), "direct-edit didClose reload failed");
                                    }
                                    if let Ok(mut files) = view.direct_edit_open_files.lock() {
                                        files.remove(&file_path);
                                    }
                                } else {
                                    if let Err(e) =
                                        engine.clear_session_overlay(view.session_id, &file_path)
                                    {
                                        tracing::warn!(error = %e, file = %file_path.display(), "session overlay close failed");
                                    }
                                }
                            }
                            return Flow::Next;
                        }
                        // A request the in-memory engine has no answer for is refused rather
                        // than left without a reply, which an editor waits on for good.
                        Some(m) if id.as_ref().is_some_and(|i| !i.is_null()) => {
                            let refused = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32601, "message": format!("{m} is not supported by prod-code for Rust") }
                            });
                            let _ = out_tx
                                .send(WireMessage::LspPayload(refused.to_string()))
                                .await;
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
                                        send_busy_note(&mut resp, &out_tx_task).await;
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
                                    send_busy_note(&mut resp, &out_tx_task).await;
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
                            let uri = params
                                .get("textDocument")
                                .and_then(|t| t.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("")
                                .to_string();
                            let resp = match ManagedLsp::Generic(&engine)
                                .diagnostics_for(&uri)
                                .await
                            {
                                Ok(items) => {
                                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "result": { "kind": "full", "items": items } })
                                }
                                // No report is not a clean one (#471).
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
                                    send_busy_note(&mut resp, &out_tx_task).await;
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
                        let _ = generic_eng
                            .send_session_notification(view.session_id, m, params)
                            .await;
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
            let mut watched = Vec::new();
            let mut failed: Vec<String> = Vec::new();

            for delta in &req.files {
                let target_path = view.workspace.root.join(&delta.relative_path);
                match &delta.content {
                    Some(content_bytes) => {
                        bytes_transferred += content_bytes.len();
                        let kind = if target_path.exists() {
                            workspace::WatchedChange::Changed
                        } else {
                            workspace::WatchedChange::Created
                        };
                        if let Err(e) =
                            write_synced_file(&target_path, content_bytes, delta.is_executable)
                                .await
                        {
                            tracing::warn!(error = %e, file = %target_path.display(), "sync write failed; the client sends it again");
                            failed.push(delta.relative_path.clone());
                            continue;
                        }
                        files_updated += 1;
                        watched.push((target_path.clone(), kind));
                        // The workspace is this worktree's own: synced files are its
                        // new base, visible to every session except one that still
                        // holds an unsaved buffer for the same path.
                        if let Ok(text) = std::str::from_utf8(content_bytes) {
                            for engine_lock in view.workspace.mirrored_rust_engines() {
                                let mut engine = engine_lock.lock().await;
                                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    engine.update_base(&target_path, Some(text.to_string()))
                                }));
                                match res {
                                    Ok(Err(e)) => {
                                        tracing::warn!(error = %e, file = %target_path.display(), "base update failed");
                                    }
                                    Err(_) => {
                                        tracing::warn!(file = %target_path.display(), "base update panicked; continuing");
                                    }
                                    Ok(Ok(())) => {}
                                }
                            }
                        }
                    }
                    None => {
                        if target_path.exists()
                            && tokio::fs::remove_file(&target_path).await.is_ok()
                        {
                            files_deleted += 1;
                            watched.push((target_path.clone(), workspace::WatchedChange::Deleted));
                            prune_empty_parents(&view.workspace.root, target_path.parent());
                        }
                        for engine_lock in view.workspace.mirrored_rust_engines() {
                            let mut engine = engine_lock.lock().await;
                            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                engine.update_base(&target_path, None)
                            }));
                            match res {
                                Ok(Err(e)) => {
                                    tracing::warn!(error = %e, file = %target_path.display(), "base removal failed");
                                }
                                Err(_) => {
                                    tracing::warn!(file = %target_path.display(), "base removal panicked; continuing");
                                }
                                Ok(Ok(())) => {}
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

            let mut stale_paths = workspace::clear_stale_paths(
                &view.workspace.root,
                req.files.iter().map(|delta| delta.relative_path.as_str()),
            );
            workspace::record_stale_paths(&view.workspace.root, &failed);
            for path in failed {
                if !stale_paths.contains(&path) {
                    stale_paths.push(path);
                }
            }
            view.workspace.notify_watched_files(&watched).await;
            let duration_ms = start.elapsed().as_millis() as u64;
            let _ = out_tx
                .send(WireMessage::SyncResponse(SyncResponse {
                    files_updated,
                    files_deleted,
                    bytes_transferred,
                    duration_ms,
                    server_workspace_root: view.workspace.root.to_string_lossy().to_string(),
                    workspace_was_fresh: false,
                    stale_paths,
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
                    platform: Some(prod_code_protocol::platform()),
                    running_commands: running_commands(),
                    host: memory::host_resources(&view.worktree_root),
                    version: Some(env!("CARGO_PKG_VERSION").to_string()),
                    git_commit: Some(prod_code_protocol::git_commit().to_string())
                        .filter(|c| c != "unknown"),
                }))
                .await;
        }
        Some(Ok(WireMessage::ReadFileRequest(req))) => {
            let resp = read_server_file(&meta.storage_root, &req);
            let _ = out_tx.send(WireMessage::ReadFileResponse(resp)).await;
        }
        Some(Err(e)) => {
            tracing::error!(error = %e, "TCP frame decode error");
            return Flow::Stop;
        }
        None => {
            tracing::debug!("Client disconnected");
            return Flow::Stop;
        }
        _ => {}
    }
    Flow::Next
}

fn lsp_call_hierarchy(
    out_tx: &SharedOutputSender,
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
    let (line, col) = one_based_position(position).unwrap_or((1, 1));
    let file_path = uri_or_path(uri);
    let method_name = hm.to_string();

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(req = req_num, session = view.session_id, method = %method_name, file = %file_path.display(), pos = format!("{line}:{col}"), in_flight, "🚀 [LSP START]");

    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let m = method_name.clone();
        let fp = fp_clone.clone();
        let outcome = execute_bounded_query(
            &engine_arc,
            session_id,
            &fp_clone,
            is_single_owner,
            move |snapshot| hierarchy_query(snapshot, &m, &fp, line, col),
        )
        .await;
        let outcome = match outcome {
            Err(ref e)
                if method_name == "textDocument/diagnostic"
                    && e.to_string().starts_with("analyzer panic: ") =>
            {
                let msg = e.to_string();
                let msg = msg.strip_prefix("analyzer panic: ").unwrap_or(&msg);
                Ok(analyzer_panic_report(msg))
            }
            other => other,
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
    out_tx: &SharedOutputSender,
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
    let (line, col) = one_based_position(params.get("position")).unwrap_or((1, 1));
    let file_path = uri_or_path(uri);
    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(req = req_num, session = view.session_id, method = "prodCode/safeDelete", file = %file_path.display(), pos = format!("{line}:{col}"), in_flight, "🚀 [LSP START]");
    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();
    tokio::task::spawn(async move {
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                engine.safe_delete(&fp_clone, line, col)
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
    out_tx: &SharedOutputSender,
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
    let (line, col) = params
        .get("position")
        .map(|position| one_based_position(Some(position)))
        .transpose()
        .unwrap_or(Some((1, 1)))
        .unwrap_or((1, 1));
    let rule = params
        .get("rule")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    let scope = params
        .get("scope")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(uri_or_path);
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "prodCode/structuralReplace",
        file = %file_path.display(),
        pos = format!("{line}:{col}"),
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
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                engine.structural_replace(&rule, &fp_clone, line, col, scope.as_deref())
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
    out_tx: &SharedOutputSender,
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
    let (line, col) = one_based_position(params.get("position")).unwrap_or((1, 1));
    let new_name = params
        .get("newName")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/rename",
        file = %file_path.display(),
        pos = format!("{line}:{col}"),
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
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                engine.rename(&fp_clone, line, col, &new_name)
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
    out_tx: &SharedOutputSender,
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
    let (line, col) = one_based_position(params.pointer("/range/start")).unwrap_or((1, 1));
    let end = params
        .pointer("/range/end")
        .and_then(|end| one_based_position(Some(end)).ok());
    let assist_id = params
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let subtype = params
        .get("subtype")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    let method_name = if apply {
        "prodCode/applyAssist"
    } else {
        "prodCode/assists"
    };
    tracing::info!(req = req_num, session = view.session_id, method = method_name, file = %file_path.display(), pos = format!("{line}:{col}"), assist = %assist_id, in_flight, "🚀 [LSP START]");

    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let result = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                if apply {
                    engine
                        .apply_assist(&fp_clone, line, col, end, &assist_id, subtype)
                        .map(|r| r.map(|outcome| workspace_edit_json(&outcome)))
                } else {
                    engine
                        .list_assists(&fp_clone, line, col, end)
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
    out_tx: &SharedOutputSender,
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
    let ws_root = view.workspace.root.clone();
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let q = query.clone();
        let syms = execute_bounded_query(
            &engine_arc,
            session_id,
            &ws_root,
            is_single_owner,
            move |snapshot| snapshot.workspace_symbols(&q, limit),
        )
        .await
        .unwrap_or_default();
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
                    "uri": file_uri(&s.path),
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
    out_tx: &SharedOutputSender,
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
    let file_path = uri_or_path(&uri);

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
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let fp = fp_clone.clone();
        let syms = execute_bounded_query(
            &engine_arc,
            session_id,
            &fp_clone,
            is_single_owner,
            move |snapshot| {
                // An error is the answer, not an empty file: a README in a Rust workspace is
                // refused with the reason, and an agent must be able to tell that from a file
                // that declares nothing (#270).
                snapshot.document_symbols(&fp).map_err(|e| {
                    tracing::warn!(error = %e, session = session_id, "query failed");
                    e
                })
            },
        )
        .await
        .map_err(|e| e.to_string());

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let count = syms.as_ref().map_or(0, Vec::len);

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

        let syms = match syms {
            Ok(syms) => syms,
            Err(message) => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "error": { "code": -32603, "message": message }
                });
                let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                return;
            }
        };
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
    out_tx: &SharedOutputSender,
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
    let (line, col) = one_based_position(params.get("position")).unwrap_or((1, 1));
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();

    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/references",
        file = %file_path.display(),
        pos = format!("{line}:{col}"),
        in_flight,
        "🚀 [LSP START]"
    );

    let engine_arc = Arc::clone(engine_lock);

    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let refs = execute_bounded_query(&engine_arc, session_id, &fp_clone, is_single_owner, {
            let fp = fp_clone.clone();
            move |snapshot| snapshot.find_all_refs(&fp, line, col)
        })
        .await;

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let count = refs.as_ref().map_or(0, Vec::len);

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

        let locations = refs.map(|targets| targets.into_iter().map(|t| {
            serde_json::json!({
                "uri": file_uri(&t.path),
                "range": {
                    "start": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) },
                    "end": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) }
                }
            })
        }).collect::<Vec<_>>());

        let resp = match locations {
            Ok(locations) => {
                serde_json::json!({"jsonrpc": "2.0", "id": req_id, "result": locations})
            }
            Err(error) => serde_json::json!({
                "jsonrpc": "2.0", "id": req_id,
                "error": { "code": -32603, "message": error.to_string() }
            }),
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

fn lsp_definition(
    out_tx: &SharedOutputSender,
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
    let (line, col) = one_based_position(params.get("position")).unwrap_or((1, 1));
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();

    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/definition",
        file = %file_path.display(),
        pos = format!("{line}:{col}"),
        in_flight,
        "🚀 [LSP START]"
    );

    let engine_arc = Arc::clone(engine_lock);

    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let defs = execute_bounded_query(&engine_arc, session_id, &fp_clone, is_single_owner, {
            let fp = fp_clone.clone();
            move |snapshot| snapshot.goto_definition(&fp, line, col)
        })
        .await;

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let count = defs.as_ref().map_or(0, Vec::len);

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

        let locations = defs.map(|targets| targets.into_iter().map(|t| {
            serde_json::json!({
                "uri": file_uri(&t.path),
                "range": {
                    "start": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) },
                    "end": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) }
                }
            })
        }).collect::<Vec<_>>());

        let resp = match locations {
            Ok(locations) => {
                serde_json::json!({"jsonrpc": "2.0", "id": req_id, "result": locations})
            }
            Err(error) => serde_json::json!({
                "jsonrpc": "2.0", "id": req_id,
                "error": { "code": -32603, "message": error.to_string() }
            }),
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

fn lsp_hover(
    out_tx: &SharedOutputSender,
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
    let (line, col) = one_based_position(params.get("position")).unwrap_or((1, 1));
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();

    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/hover",
        file = %file_path.display(),
        pos = format!("{line}:{col}"),
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
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let hover_res = execute_bounded_query(&engine_arc, session_id, &fp_clone, is_single_owner, {
            let fp = fp_clone.clone();
            move |snapshot| snapshot.hover(&fp, line, col)
        })
        .await;

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let found = matches!(&hover_res, Ok(Some(_)));

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
            Ok(Some(markup)) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "contents": {
                        "kind": "markdown",
                        "value": markup
                    }
                }
            }),
            Ok(None) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": null
            }),
            Err(error) => serde_json::json!({
                "jsonrpc": "2.0", "id": req_id,
                "error": { "code": -32603, "message": error.to_string() }
            }),
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

/// One of the requests an editor needs beyond navigation (completion, signature help, inlay
/// hints, highlights, code actions, formatting), answered by the in-memory Rust engine in the
/// session's view (#310).
fn lsp_editor_request(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    method: &str,
    params: serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();
    let method = method.to_string();
    let started = Instant::now();
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    tokio::task::spawn(async move {
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            let m = method.clone();
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                engine
                    .editor_request(&m, &params)
                    .unwrap_or_else(|| Err(anyhow::anyhow!("{m} is not an editor request")))
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("query task failed: {e}")))
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let resp = match outcome {
            Ok(result) => {
                tracing::info!(session = session_id, method = %method, duration_ms = format!("{ms:.2}ms"), "✅ [LSP DONE]");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": result })
            }
            Err(e) => {
                tracing::warn!(session = session_id, method = %method, error = %format!("{e:#}"), "editor request failed");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": format!("{e:#}") } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

/// How long an editor session's document must stay unchanged before its diagnostics are
/// computed: a burst of typing costs one pass, for its last edit.
const EDITOR_DIAGNOSTICS_DELAY: Duration = Duration::from_millis(300);

/// Pushes the diagnostics of `path` to an editor session once the document has stopped
/// changing for [`EDITOR_DIAGNOSTICS_DELAY`] (#310). Sessions of agents and tools ask for
/// diagnostics when they want them and get none pushed.
fn publish_rust_diagnostics(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    meta: &Arc<SessionMeta>,
    path: PathBuf,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    if !meta.editor {
        return;
    }
    let edit = {
        let mut edits = meta.edits.lock().unwrap_or_else(|e| e.into_inner());
        let count = edits.entry(path.clone()).or_default();
        *count += 1;
        *count
    };
    let edits = Arc::clone(&meta.edits);
    let engine_arc = Arc::clone(engine_lock);
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();
    tokio::task::spawn(async move {
        tokio::time::sleep(EDITOR_DIAGNOSTICS_DELAY).await;
        let latest = |edits: &std::sync::Mutex<std::collections::HashMap<PathBuf, u64>>| {
            edits
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&path)
                .copied()
        };
        if latest(&edits) != Some(edit) {
            return;
        }
        let file = path.clone();
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                engine.editor_diagnostics(&file)
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("diagnostics task failed: {e}")))
        };
        // An edit that arrived during the pass gets a pass of its own.
        if latest(&edits) != Some(edit) {
            return;
        }
        match outcome {
            Ok(diagnostics) => {
                let note = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": { "uri": file_uri(&path), "diagnostics": diagnostics }
                });
                let client_note = translator_task.translate_lsp_to_client(&note.to_string());
                let _ = out_tx_task.send(WireMessage::LspPayload(client_note)).await;
            }
            Err(e) => {
                tracing::warn!(session = session_id, file = %path.display(), error = %format!("{e:#}"), "editor diagnostics failed");
            }
        }
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
    for engine in [
        "cpp", "swift", "python", "typescript", "java", "kotlin", "csharp", "scala", "php", "ruby", "dart",
        "zig",
    ] {
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
    let mut unconfirmed_since: std::collections::HashMap<String, Instant> =
        std::collections::HashMap::new();

    loop {
        tokio::time::sleep(GOSSIP_PERIOD).await;

        // 1. Evict silent peers from cluster that haven't sent heartbeats in PEER_EVICT
        {
            let mut cluster = state.cluster.write().await;
            let mut evicted = Vec::new();
            cluster.retain(|addr, entry| {
                if entry.last_seen.elapsed() >= PEER_EVICT {
                    evicted.push(addr.clone());
                    false
                } else {
                    true
                }
            });
            if !evicted.is_empty() {
                let mut peers = state.peers.write().await;
                for dead in &evicted {
                    peers.remove(dead);
                    unconfirmed_since.remove(dead);
                }
                tracing::info!(evicted = ?evicted, "evicted silent peers from cluster");
            }
        }

        // 2. Evict unconfirmed transitive peers from state.peers that never connected within PEER_EVICT
        {
            let cluster = state.cluster.read().await;
            let mut peers = state.peers.write().await;
            let now = Instant::now();
            let mut dead_unconfirmed = Vec::new();
            for p in peers.iter() {
                if !cluster.contains_key(p) {
                    let first_seen = unconfirmed_since.entry(p.clone()).or_insert(now);
                    if now.duration_since(*first_seen) >= PEER_EVICT {
                        dead_unconfirmed.push(p.clone());
                    }
                } else {
                    unconfirmed_since.remove(p);
                }
            }
            for d in &dead_unconfirmed {
                peers.remove(d);
                unconfirmed_since.remove(d);
            }
            if !dead_unconfirmed.is_empty() {
                tracing::info!(evicted = ?dead_unconfirmed, "evicted unreachable transitive peers");
            }
        }

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
                    // Peers share the cluster's token with the clients (#402).
                    let stream = prod_code_protocol::transport::connect_stream_with_client_config(
                        addr,
                        state.auth_token.as_deref(),
                        state.client_tls.clone(),
                    )
                    .await
                    .ok()?;
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

/// UDP discovery: listens for probe packets (multicast and unicast), replies with the full
/// cluster view, and periodically announces this node on multicast.
async fn discovery_loop(state: Arc<ServerState>) {
    use prod_code_protocol::discovery;

    let sock = match discovery::bind_discovery_socket() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%e, "UDP discovery socket bind failed; discovery disabled");
            return;
        }
    };
    let tok_sock = match tokio::net::UdpSocket::from_std(sock) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%e, "failed to register discovery socket with tokio");
            return;
        }
    };
    tracing::info!(port = discovery::DISCOVERY_PORT, "UDP discovery listener started");

    let mut announce_tick = tokio::time::interval(discovery::ANNOUNCE_PERIOD);
    let mut buf = vec![0u8; 4096];

    loop {
        tokio::select! {
            // Incoming datagram: probe or peer announce.
            result = tok_sock.recv_from(&mut buf) => {
                match result {
                    Ok((n, from)) => {
                        let token = state.auth_token.as_deref();
                        if let Some(probe_nonce) = discovery::inspect_probe(&buf[..n], token) {
                            let own_line = build_own_announce(&state, probe_nonce.as_deref()).await;
                            let cluster = state.cluster.read().await;
                            let peer_lines: Vec<String> = cluster
                                .values()
                                .filter(|e| e.last_seen.elapsed() < std::time::Duration::from_secs(30))
                                .map(|e| build_peer_announce(e, token, probe_nonce.as_deref()))
                                .collect();
                            drop(cluster);
                            let payload = discovery::build_reply(&own_line, &peer_lines);
                            let _ = tok_sock.send_to(&payload, from).await;
                        } else if let Some(dns_resp) = handle_gateway_dns_query(&buf[..n], &state).await {
                            let _ = tok_sock.send_to(&dns_resp, from).await;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        tracing::debug!(%e, "discovery recv error");
                    }
                }
            }
            // Periodic multicast announce: minimal announcement for LAN discovery privacy (Phase 5.6).
            _ = announce_tick.tick() => {
                let line = build_own_minimal_announce(&state).await;
                let payload = discovery::build_reply(&line, &[]);
                let _ = tok_sock.send_to(
                    &payload,
                    std::net::SocketAddrV4::new(discovery::MULTICAST_GROUP, discovery::DISCOVERY_PORT),
                ).await;
            }
        }
    }
}

/// Handles incoming DNS queries (*.code.internal, _prod-code._tcp) over UDP on port 9401 (Phase 5.2).
async fn handle_gateway_dns_query(buf: &[u8], state: &ServerState) -> Option<Vec<u8>> {
    use prod_code_protocol::dns::handle_dns_packet;
    use prod_code_protocol::discovery::{DiscoveredNode, LoadedWorkspace};

    let own_advertise = state.advertise.read().await.clone();
    let own_addr: SocketAddr = own_advertise.parse().ok()?;
    let own_engines: Vec<String> = state
        .advertised_engines()
        .iter()
        .map(|e| e.split(' ').next().unwrap_or(e).to_string())
        .collect();
    let status = state.status().await;
    let ws_summary = state.workspace_manager.loaded_summary().await;
    let own_workspaces: Vec<LoadedWorkspace> = ws_summary
        .into_iter()
        .map(|(name, engine, sessions)| LoadedWorkspace {
            name,
            engine,
            sessions: sessions as u32,
        })
        .collect();

    let mut nodes = vec![DiscoveredNode {
        addr: own_addr,
        engines: own_engines,
        rss_mb: status.memory_rss_bytes.unwrap_or(0) / (1024 * 1024),
        load_per_cpu: status.load_per_cpu().unwrap_or(0.0),
        cpus: status.cpu_count.unwrap_or(0) as u32,
        mem_total_mb: status.host.memory_total_bytes.unwrap_or(0) / (1024 * 1024),
        mem_avail_mb: status.host.memory_available_bytes.unwrap_or(0) / (1024 * 1024),
        sessions: state.active_sessions.load(std::sync::atomic::Ordering::Relaxed) as u32,
        workspaces: own_workspaces,
        nonce: None,
    }];

    let cluster = state.cluster.read().await;
    for entry in cluster.values() {
        if entry.last_seen.elapsed() < std::time::Duration::from_secs(30) {
            if let Ok(addr) = entry.gossip.addr.parse::<SocketAddr>() {
                let eng: Vec<String> = entry
                    .gossip
                    .status
                    .detected_engines
                    .iter()
                    .map(|en| en.split(' ').next().unwrap_or(en).to_string())
                    .collect();
                let peer_workspaces: Vec<LoadedWorkspace> = entry
                    .gossip
                    .workspaces
                    .iter()
                    .map(|w| LoadedWorkspace {
                        name: w.name.clone(),
                        engine: w.engine.clone(),
                        sessions: w.sessions as u32,
                    })
                    .collect();
                nodes.push(DiscoveredNode {
                    addr,
                    engines: eng,
                    rss_mb: entry.gossip.status.memory_rss_bytes.unwrap_or(0) / (1024 * 1024),
                    load_per_cpu: entry.gossip.status.load_per_cpu().unwrap_or(0.0),
                    cpus: entry.gossip.status.cpu_count.unwrap_or(0) as u32,
                    mem_total_mb: entry.gossip.status.host.memory_total_bytes.unwrap_or(0) / (1024 * 1024),
                    mem_avail_mb: entry.gossip.status.host.memory_available_bytes.unwrap_or(0) / (1024 * 1024),
                    sessions: entry.gossip.workspaces.iter().map(|w| w.sessions as u32).sum(),
                    workspaces: peer_workspaces,
                    nonce: None,
                });
            }
        }
    }
    drop(cluster);

    nodes.sort_by_key(|n| n.addr);
    handle_dns_packet(buf, &nodes)
}

/// Build this node's minimal discovery announce line for privacy (endpoint + engines only).
async fn build_own_minimal_announce(state: &ServerState) -> String {
    use prod_code_protocol::discovery;
    let advertise = state.advertise.read().await.clone();
    let engines_csv = state
        .advertised_engines()
        .iter()
        .map(|e| e.split(' ').next().unwrap_or(e).to_string())
        .collect::<Vec<_>>()
        .join(",");
    discovery::format_minimal_node_line(
        &advertise,
        &engines_csv,
        state.auth_token.as_deref(),
    )
}

/// Build this node's discovery announce line with full routing metadata and optional challenge nonce echo.
async fn build_own_announce(state: &ServerState, nonce: Option<&str>) -> String {
    use prod_code_protocol::discovery;
    let advertise = state.advertise.read().await.clone();
    let engines_csv = state
        .advertised_engines()
        .iter()
        .map(|e| e.split(' ').next().unwrap_or(e).to_string())
        .collect::<Vec<_>>()
        .join(",");
    let status = state.status().await;
    let ws_summary = state.workspace_manager.loaded_summary().await;
    let workspaces: Vec<(String, String, u32)> = ws_summary
        .into_iter()
        .map(|(name, engine, sessions)| (name, engine, sessions as u32))
        .collect();
    discovery::format_node_line_with_nonce(
        &advertise,
        &engines_csv,
        status.memory_rss_bytes.unwrap_or(0) / (1024 * 1024),
        status.load_per_cpu().unwrap_or(0.0),
        status.cpu_count.unwrap_or(0) as u32,
        status.host.memory_total_bytes.unwrap_or(0) / (1024 * 1024),
        status.host.memory_available_bytes.unwrap_or(0) / (1024 * 1024),
        state.active_sessions.load(std::sync::atomic::Ordering::Relaxed) as u32,
        &workspaces,
        state.auth_token.as_deref(),
        nonce,
    )
}

/// Build a peer's discovery announce line from its gossip data with optional challenge nonce echo.
fn build_peer_announce(entry: &PeerEntry, token: Option<&str>, nonce: Option<&str>) -> String {
    use prod_code_protocol::discovery;
    let eng = entry
        .gossip
        .status
        .detected_engines
        .iter()
        .map(|en| en.split(' ').next().unwrap_or(en).to_string())
        .collect::<Vec<_>>()
        .join(",");
    let workspaces: Vec<(String, String, u32)> = entry
        .gossip
        .workspaces
        .iter()
        .map(|w| (w.name.clone(), w.engine.clone(), w.sessions as u32))
        .collect();
    discovery::format_node_line_with_nonce(
        &entry.gossip.addr,
        &eng,
        entry.gossip.status.memory_rss_bytes.unwrap_or(0) / (1024 * 1024),
        entry.gossip.status.load_per_cpu().unwrap_or(0.0),
        entry.gossip.status.cpu_count.unwrap_or(0) as u32,
        entry.gossip.status.host.memory_total_bytes.unwrap_or(0) / (1024 * 1024),
        entry.gossip.status.host.memory_available_bytes.unwrap_or(0) / (1024 * 1024),
        entry.gossip.workspaces.iter().map(|w| w.sessions as u32).sum::<u32>(),
        &workspaces,
        token,
        nonce,
    )
}

/// How long an engine may sit idle on a host short of memory before it is unloaded, however
/// long `--idle-evict-secs` lets it stay otherwise (#396).
const PRESSURE_EVICT_IDLE: Duration = Duration::from_secs(300);

/// After how long idle engines are unloaded: `--idle-evict-secs` (0 keeps them), cut to
/// [`PRESSURE_EVICT_IDLE`] while the host is short of memory, even when eviction is off.
fn evict_after(idle_evict_secs: u64, memory_short: bool) -> Option<Duration> {
    let configured = (idle_evict_secs > 0).then(|| Duration::from_secs(idle_evict_secs));
    if memory_short {
        Some(configured.map_or(PRESSURE_EVICT_IDLE, |c| c.min(PRESSURE_EVICT_IDLE)))
    } else {
        configured
    }
}

async fn janitor(
    state: Arc<ServerState>,
    idle_evict_secs: u64,
    prune_worktree_secs: u64,
    prune_workspace_secs: u64,
    prune_below_free_percent: u64,
) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
    ticker.tick().await;
    let mut was_short: Option<String> = None;
    loop {
        ticker.tick().await;
        // An engine installed while the daemon runs is picked up here, on a thread that is
        // allowed to block, instead of by the next request that needs the list.
        let _ = tokio::task::spawn_blocking(refresh_available_engines).await;
        // The memory and disk watchdog (#396): placement already keeps new workspaces off a
        // node that is short; the log says when it starts and stops being short.
        let host = memory::host_resources(&state.storage_root);
        let short = host.pressure();
        match (&was_short, &short) {
            (None, Some(why)) => tracing::warn!(
                %why,
                "⚠️ [PRESSURE] this node is short: new workspaces go to other nodes"
            ),
            (Some(_), None) => {
                tracing::info!(now = %host.describe(), "✅ [PRESSURE] this node has room again")
            }
            _ => {}
        }
        was_short = short;
        if was_short.is_some() {
            let view = state.cluster_view().await;
            let own_addr = state.advertise.read().await.clone();
            let ws_summary = state.workspace_manager.loaded_workspaces_for_rebalance().await;
            for (ws, active) in ws_summary {
                if active > 0 {
                    let ws_name = ws
                        .root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let os = if ws.engine == "swift" {
                        Some("macos".to_string())
                    } else {
                        None
                    };
                    let place_req = PlaceRequest {
                        workspace_name: ws_name,
                        engine: Some(ws.engine.clone()),
                        os,
                        rebalance_active: true,
                    };
                    let place_resp = place_in(&place_req, view.clone());
                    if let Some(target) = place_resp.node {
                        if target != own_addr && target != view.this_node {
                            let notified = ws.trigger_rebalance(
                                target.clone(),
                                Some("evacuating node under memory pressure".to_string()),
                            );
                            if notified > 0 {
                                tracing::info!(
                                    workspace = %ws.root.display(),
                                    target = %target,
                                    notified,
                                    "rebalanced active workspace to compatible peer under host pressure"
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }
        let memory_short = host
            .memory_used_share()
            .is_some_and(|used| used > prod_code_protocol::MEMORY_PRESSURE_USED);
        if let Some(after) = evict_after(idle_evict_secs, memory_short) {
            let evicted = state.workspace_manager.evict_idle(after).await;
            for root in evicted {
                tracing::info!(workspace = %root.display(), idle_secs = after.as_secs(), memory_short, "💤 [EVICT] unloaded idle workspace engine");
            }
        }
        if prune_worktree_secs > 0 {
            let pruned = workspace::prune_stale_worktree_dirs(
                &state.storage_root,
                std::time::Duration::from_secs(prune_worktree_secs),
                &state.workspace_manager,
            )
            .await;
            for path in pruned {
                state.search_indexes.forget(&path);
            }
        }
        if prune_workspace_secs > 0 {
            let pruned = workspace::prune_stale_main_workspace_dirs(
                &state.storage_root,
                std::time::Duration::from_secs(prune_workspace_secs),
                std::time::Duration::from_secs(prune_worktree_secs),
                &state.workspace_manager,
            )
            .await;
            for path in pruned {
                state.search_indexes.forget(&path);
            }
        }
        if prune_below_free_percent > 0 {
            let pruned = workspace::prune_worktree_dirs_for_space(
                &state.storage_root,
                prune_below_free_percent as f64 / 100.0,
                &state.workspace_manager,
                workspace::free_share,
            )
            .await;
            for path in pruned {
                state.search_indexes.forget(&path);
            }
        }
        if state.build_cache_ram {
            let build_cache_base = state.build_cache_dir.clone().unwrap_or_else(|| {
                if Path::new("/dev/shm").is_dir() {
                    PathBuf::from("/dev/shm/prod-code-build")
                } else {
                    PathBuf::from("/tmp/prod-code-build")
                }
            });
            let _ = tokio::task::spawn_blocking(move || sweep_ram_build_caches(&build_cache_base)).await;
        }
        let _ = tokio::task::spawn_blocking(|| {
            let _ = python_cache::prune_stale_stub_cache(
                std::time::Duration::from_secs(7 * 86400),
                5 * 1024 * 1024 * 1024,
            );
            let _ = swift_cache::prune_stale_module_cache(
                std::time::Duration::from_secs(7 * 86400),
                10 * 1024 * 1024 * 1024,
            );
            let _ = ts_cache::prune_stale_types_cache(
                std::time::Duration::from_secs(7 * 86400),
                5 * 1024 * 1024 * 1024,
            );
        })
        .await;
    }
}

/// Runs the gateway: bind, serve, and return when a signal says to stop.
pub async fn run(cli: ServerCli) -> Result<()> {
    ensure_blocking_stdio();
    prefer_rustup_toolchain();
    let shadow_root = cli
        .shadow_dir
        .clone()
        .unwrap_or_else(|| shadow::default_root(&cli.storage));
    // Ownership precedes cleanup: no startup may sweep another live gateway's hypotheses.
    // Transfer the guard into shared state so accepted sessions retain it after this frame returns.
    let shadow_owner = shadow::ShadowRootOwner::acquire(&shadow_root)?;
    let swept = shadow_owner.sweep();
    if swept > 0 {
        tracing::info!(dir = %shadow_root.display(), swept, "removed leftover shadow directories");
    }
    // Probe once here, while nothing is waiting on us, rather than on the first request.
    let engines = refresh_available_engines();
    tracing::info!(?engines, "engines detected");

    let _ = tokio::task::spawn_blocking(|| {
        let _ = python_cache::prune_stale_stub_cache(
            std::time::Duration::from_secs(7 * 86400),
            5 * 1024 * 1024 * 1024,
        );
        let _ = swift_cache::prune_stale_module_cache(
            std::time::Duration::from_secs(7 * 86400),
            10 * 1024 * 1024 * 1024,
        );
        let _ = ts_cache::prune_stale_types_cache(
            std::time::Duration::from_secs(7 * 86400),
            5 * 1024 * 1024 * 1024,
        );
    })
    .await;

    tracing::info!(
        "prod-code gateway daemon starting on {} (storage: {:?})",
        cli.bind,
        cli.storage
    );

    let prune_worktree_secs = cli.effective_prune_worktree_secs();
    let prune_workspace_secs = cli.effective_prune_workspace_secs();
    let mut state = ServerState::new(cli.storage);
    state.build_cache_ram = cli.build_cache_ram;
    state.build_cache_dir = cli.build_cache_dir;
    if state.build_cache_ram {
        let build_cache_base = state.build_cache_dir.clone().unwrap_or_else(|| {
            if Path::new("/dev/shm").is_dir() {
                PathBuf::from("/dev/shm/prod-code-build")
            } else {
                PathBuf::from("/tmp/prod-code-build")
            }
        });
        sweep_ram_build_caches(&build_cache_base);
    }
    state.workspace_manager = Arc::new(WorkspaceManager::with_admission_and_concurrency(
        Arc::new(admission::Admission::host(cli.engine_reserve_mib)),
        cli.max_concurrent_engine_loads,
    ));
    state.engine_allowlist = cli
        .engines
        .iter()
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    if !state.engine_allowlist.is_empty() {
        tracing::info!(engines = ?state.engine_allowlist, "serving only the listed engines");
    }
    // The same token the clients send, from the same variables; its value is never logged.
    state.auth_token = prod_code_protocol::transport::auth_token();
    tracing::info!(
        required = state.auth_token.is_some(),
        "connection token (PROD_CODE_AUTH_TOKEN or PROD_CODE_AUTH_TOKEN_FILE)"
    );
    state.shadow_root = shadow_root;
    // Accepted sessions retain an Arc<ServerState>, so they must also retain exclusive ownership
    // after the accept loop returns on a shutdown signal.
    state._shadow_root_owner = Some(shadow_owner);
    match shadow::overlay_unavailable() {
        None => {
            tracing::info!(dir = %state.shadow_root.display(), "shadow runs: overlay mode (user namespaces + overlayfs)")
        }
        Some(reason) => tracing::info!(reason, "shadow runs: in-place mode"),
    }

    let tls_mode = prod_code_protocol::tls::TlsMode::from_env()?;
    let server_tls = prod_code_protocol::tls::ServerTlsConfig::from_env()?;
    let tls_acceptor = if let Some(tls_cfg) = server_tls {
        Some(tls_cfg.build_acceptor()?)
    } else {
        None
    };

    // Load and build outbound client TLS configuration before scrubbing TLS_KEY_ENV (#Phase 5.6):
    let client_tls_built = match prod_code_protocol::tls::ClientTlsConfig::from_env()? {
        Some(cfg) => Some(cfg.build()?),
        None => None,
    };
    prod_code_protocol::transport::set_default_client_tls_built(client_tls_built.clone());
    state.client_tls = client_tls_built;

    if tls_mode.is_required() && tls_acceptor.is_none() {
        return Err(anyhow::anyhow!(
            "TLS mode {:?} is strictly required, but server TLS credentials/CA are not configured",
            tls_mode
        ));
    }
    tracing::info!(
        mode = ?tls_mode,
        tls_enabled = tls_acceptor.is_some(),
        "cluster transport security (Phase 5.6)"
    );

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
    tokio::spawn(discovery_loop(Arc::clone(&state)));
    if let Some(rx) = state.metrics.take_receiver() {
        tokio::spawn(metrics::run_writer(state.metrics.dir().to_path_buf(), rx));
    }

    tokio::spawn(janitor(
        Arc::clone(&state),
        cli.idle_evict_secs,
        prune_worktree_secs,
        prune_workspace_secs,
        cli.prune_below_free_percent,
    ));

    #[cfg(unix)]
    let unix_listener = if let Some(ref path) = cli.socket_path {
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let u_listener = tokio::net::UnixListener::bind(path)?;
        tracing::info!("prod-code gateway listening on unix socket {}", path.display());
        Some(u_listener)
    } else {
        None
    };

    #[cfg(unix)]
    struct SocketCleaner<'a>(Option<&'a Path>);
    #[cfg(unix)]
    impl Drop for SocketCleaner<'_> {
        fn drop(&mut self) {
            if let Some(p) = self.0 {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    #[cfg(unix)]
    let _cleaner = SocketCleaner(cli.socket_path.as_deref());

    // A gateway is stopped by its supervisor (launchd, systemd) and by a deploy script, both
    // of which send SIGTERM and then wait. Without a handler the process dies where it stands:
    // in-flight queries are cut, and nothing that runs at exit runs. Stopping the accept loop
    // and returning normally is all that is needed — every session is a task holding its own
    // socket, and the client treats a closed connection as a session to re-open.
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    async fn accept_unix(
        listener: &Option<tokio::net::UnixListener>,
    ) -> std::io::Result<(tokio::net::UnixStream, tokio::net::unix::SocketAddr)> {
        match listener {
            Some(l) => l.accept().await,
            None => std::future::pending().await,
        }
    }

    let handshake_limiter = Arc::new(tokio::sync::Semaphore::new(128));
    let tls_acceptor = tls_acceptor.map(Arc::new);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (socket, addr) = match accepted {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!(%e, "TCP accept error");
                        continue;
                    }
                };
                let permit = match handshake_limiter.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        tracing::warn!(%addr, "Too many concurrent handshakes; dropping connection");
                        continue;
                    }
                };
                prod_code_protocol::transport::tune(&socket);
                let state_clone = Arc::clone(&state);
                let tls_acceptor = tls_acceptor.clone();
                let addr_str = addr.to_string();

                tokio::spawn(async move {
                    let _permit = permit;
                    let stream = if let Some(ref acceptor) = tls_acceptor {
                        let mut peek_buf = [0u8; 1];
                        let peek_result = tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            socket.peek(&mut peek_buf),
                        ).await;
                        match peek_result {
                            Ok(Ok(1)) if peek_buf[0] == 0x16 => {
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(15),
                                    acceptor.accept(socket),
                                ).await {
                                    Ok(Ok(tls_stream)) => AnyStream::TlsServer(tls_stream),
                                    Ok(Err(e)) => {
                                        tracing::warn!(%addr, %e, "TLS handshake failed on accepted connection");
                                        return;
                                    }
                                    Err(_) => {
                                        tracing::warn!(%addr, "TLS handshake timed out");
                                        return;
                                    }
                                }
                            }
                            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {
                                if tls_mode.is_required() {
                                    tracing::warn!(%addr, "Plaintext connection rejected: TLS mode {:?} is strictly required", tls_mode);
                                    return;
                                }
                                AnyStream::Tcp(socket)
                            }
                        }
                    } else if tls_mode.is_required() {
                        tracing::warn!(%addr, "Connection rejected: TLS mode {:?} is required but no server TLS acceptor configured", tls_mode);
                        return;
                    } else {
                        AnyStream::Tcp(socket)
                    };

                    drop(_permit);
                    if let Err(err) = handle_client(stream, addr_str.clone(), state_clone).await {
                        tracing::error!(addr = %addr_str, %err, "Error in client connection");
                    }
                });
            }
            accepted = accept_unix(&unix_listener) => {
                let (socket, _) = match accepted {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!(%e, "Unix socket accept error");
                        continue;
                    }
                };
                let state_clone = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(err) = handle_client(AnyStream::Unix(socket), "unix-socket".to_string(), state_clone).await {
                        tracing::error!(addr = "unix-socket", %err, "Error in client connection");
                    }
                });
            }
            _ = terminate.recv() => {
                tracing::info!("SIGTERM: no longer accepting connections");
                return Ok(());
            }
            _ = interrupt.recv() => {
                tracing::info!("SIGINT: no longer accepting connections");
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// Only a server's requests are held back from the client, not its notifications (#391).
    #[test]
    fn a_server_request_is_told_from_a_notification() {
        assert!(super::is_server_request(
            r#"{"jsonrpc":"2.0","id":2,"method":"workspace/configuration","params":{}}"#
        ));
        assert!(!super::is_server_request(
            r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///a","diagnostics":[]}}"#
        ));
        assert!(!super::is_server_request(
            r#"{"jsonrpc":"2.0","id":2,"result":null}"#
        ));
    }

    use super::*;
    use prod_code_protocol::{HostResources, PROTOCOL_VERSION};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn shared_output_queue_deadline_closes_every_generation_sender() {
        let (raw_tx, _rx) = rapidfire::mpsc::bounded(1);
        let output = SharedOutputSender::new(raw_tx, Duration::from_millis(10));
        output.send(WireMessage::Ping).await.unwrap();

        assert!(matches!(
            output.send(WireMessage::Ping).await,
            Err(SharedOutputSendError::Deadline)
        ));
        assert!(matches!(
            output.send(WireMessage::Ping).await,
            Err(SharedOutputSendError::Closed)
        ));
    }

    #[tokio::test]
    async fn owned_join_aborts_and_observes_the_exact_writer_task() {
        let task = tokio::spawn(async {
            std::future::pending::<()>().await;
            Ok(())
        });
        let mut owned = OwnedJoin::new(task);
        owned.abort();
        let result = owned.task_mut().await;
        assert!(
            result
                .as_ref()
                .is_err_and(tokio::task::JoinError::is_cancelled)
        );
        assert!(flatten_writer_result(result).is_err());
        owned.clear_finished();
    }

    #[test]
    fn read_server_file_caps_external_source_to_2mib_even_with_explicit_large_limit() {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        let external_base = home.join(".cargo/registry");
        if std::fs::create_dir_all(&external_base).is_err() {
            return;
        }
        let test_file = external_base.join(format!("test_cap_{}.txt", std::process::id()));
        let data = vec![b'x'; 3 * 1024 * 1024]; // 3 MiB
        if std::fs::write(&test_file, &data).is_err() {
            return;
        }
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _guard = Cleanup(test_file.clone());

        let temp_storage = tempfile::tempdir().unwrap();

        let req = prod_code_protocol::ReadFileRequest {
            path: test_file.to_string_lossy().into_owned(),
            max_bytes: 64 * 1024 * 1024, // Explicit 64 MiB requested
        };
        let resp = read_server_file(temp_storage.path(), &req);

        assert!(resp.error.is_none(), "read_server_file failed: {:?}", resp.error);
        assert!(resp.truncated, "external source must be truncated to 2 MiB");
        let content = resp.content.expect("content present");
        assert_eq!(content.len(), 2 * 1024 * 1024, "external source capped at 2 MiB");
    }

    #[test]
    fn read_server_file_allows_workspace_artifact_up_to_64mib() {
        let temp_storage = tempfile::tempdir().unwrap();
        let artifact = temp_storage.path().join("target/release/large_bin");
        std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        let data = vec![b'y'; 3 * 1024 * 1024]; // 3 MiB
        std::fs::write(&artifact, &data).unwrap();

        let req = prod_code_protocol::ReadFileRequest {
            path: artifact.to_string_lossy().into_owned(),
            max_bytes: 0, // Default in workspace
        };
        let resp = read_server_file(temp_storage.path(), &req);
        assert!(resp.error.is_none(), "read_server_file failed: {:?}", resp.error);
        assert!(!resp.truncated, "workspace artifact must not be truncated under 64 MiB");
        assert_eq!(resp.content.expect("content").len(), 3 * 1024 * 1024);
    }

    #[test]
    fn is_readable_source_path_allows_polyglot_dependencies_and_rejects_arbitrary_files() {
        let storage = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", home.path());
        }

        // 1. Rust cargo registry
        let cargo_file = home.path().join(".cargo/registry/src/github.com/lib.rs");
        std::fs::create_dir_all(cargo_file.parent().unwrap()).unwrap();
        std::fs::write(&cargo_file, "pub fn foo() {}").unwrap();
        assert!(is_readable_source_path(storage.path(), &cargo_file));

        // 2. Python virtualenv / uv cache
        let py_file = home.path().join(".cache/uv/wheels/pkg/module.py");
        std::fs::create_dir_all(py_file.parent().unwrap()).unwrap();
        std::fs::write(&py_file, "def bar(): pass").unwrap();
        assert!(is_readable_source_path(storage.path(), &py_file));

        // 3. Node pnpm store
        let pnpm_file = home.path().join(".local/share/pnpm/store/pkg/index.d.ts");
        std::fs::create_dir_all(pnpm_file.parent().unwrap()).unwrap();
        std::fs::write(&pnpm_file, "export declare const x: number;").unwrap();
        assert!(is_readable_source_path(storage.path(), &pnpm_file));

        // 4. Any node_modules
        let nm_file = home.path().join("projects/foo/node_modules/bar/index.js");
        std::fs::create_dir_all(nm_file.parent().unwrap()).unwrap();
        std::fs::write(&nm_file, "module.exports = {};").unwrap();
        assert!(is_readable_source_path(storage.path(), &nm_file));

        // 5. Arbitrary sensitive files rejected
        let ssh_key = home.path().join(".ssh/id_rsa");
        std::fs::create_dir_all(ssh_key.parent().unwrap()).unwrap();
        std::fs::write(&ssh_key, "private-key-material").unwrap();
        assert!(!is_readable_source_path(storage.path(), &ssh_key));

        let bashrc = home.path().join(".bashrc");
        std::fs::write(&bashrc, "export SECRET=1").unwrap();
        assert!(!is_readable_source_path(storage.path(), &bashrc));
    }

    #[tokio::test]
    async fn own_gossip_does_not_propagate_unconfirmed_transitive_peers() {
        let storage = tempfile::tempdir().unwrap();
        let state = ServerState::new(storage.path().to_path_buf());
        *state.advertise.write().await = "127.0.0.1:9400".into();

        // Absorb gossip from peer A that mentions a transitive dead peer B.
        let peer_a_gossip = NodeGossip {
            addr: "127.0.0.1:9401".into(),
            status: state.status().await,
            workspaces: Vec::new(),
            peers: vec!["127.0.0.1:9402".into()], // peer B (unconfirmed)
            sent_at_ms: 1000,
        };
        state.absorb_gossip(peer_a_gossip).await;

        // own_gossip should contain peer A (which is confirmed alive by absorb_gossip),
        // but must NOT propagate peer B (which has never been directly observed alive).
        let own = state.own_gossip().await;
        assert!(
            own.peers.contains(&"127.0.0.1:9401".to_string()),
            "confirmed peer A must be gossiped"
        );
        assert!(
            !own.peers.contains(&"127.0.0.1:9402".to_string()),
            "unconfirmed peer B must NOT be propagated transitively"
        );
    }

    #[test]
    fn shadow_root_ownership_follows_the_last_server_state_reference() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("shadow");
        let mut state = ServerState::new(fixture.path().join("storage"));
        state.shadow_root = root.clone();
        state._shadow_root_owner = Some(shadow::ShadowRootOwner::acquire(&root).unwrap());

        let accept_state = Arc::new(state);
        let session_state = Arc::clone(&accept_state);
        drop(accept_state);

        let refused = shadow::ShadowRootOwner::acquire(&root)
            .err()
            .expect("a retained session state must keep ownership");
        assert!(
            refused
                .to_string()
                .contains("already owned by another gateway"),
            "{refused:#}"
        );

        drop(session_state);
        let mut acquired = shadow::ShadowRootOwner::acquire(&root);
        for _ in 0..20 {
            if acquired.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            acquired = shadow::ShadowRootOwner::acquire(&root);
        }
        acquired.expect("the final state release must release ownership");
    }

    #[tokio::test]
    async fn incompatible_protocol_offers_are_refused_before_session_or_workspace_side_effects() {
        for (name, protocol_version, supported_versions) in [
            ("empty", PROTOCOL_VERSION, Some(Vec::new())),
            ("disjoint", PROTOCOL_VERSION, Some(vec![2, 3])),
            ("legacy-unsupported", 999, None),
        ] {
            let storage = tempfile::tempdir().unwrap();
            let storage_root = storage.path().join("workspaces");
            let state = Arc::new(ServerState::new(storage_root.clone()));
            let client_root = format!("/home/dev/{name}");
            let server_root = workspace::server_workspace_path(&storage_root, &client_root, None);

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let serving = Arc::clone(&state);
            let serve = tokio::spawn(async move {
                let (socket, peer) = listener.accept().await.unwrap();
                handle_client(socket, peer, serving).await
            });

            let mut stream = TcpStream::connect(addr).await.unwrap();
            let mut payload = serde_json::json!({
                "protocol_version": protocol_version,
                "client_name": format!("{name}-offer-test"),
                "client_pid": 1,
                "auth_token": null,
                "client_workspace_root": client_root
            });
            if let Some(versions) = supported_versions {
                payload["supported_versions"] = serde_json::json!(versions);
            }
            let request = serde_json::json!({
                "type": "HandshakeRequest",
                "payload": payload
            });
            let bytes = serde_json::to_vec(&request).unwrap();
            stream
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .await
                .unwrap();
            stream.write_all(&bytes).await.unwrap();
            let mut framed = Framed::new(stream, ProdCodeCodec::new());

            let Some(Ok(WireMessage::Disconnect { reason })) = framed.next().await else {
                panic!("the {name} protocol offer was not refused");
            };
            assert!(reason.contains("protocol"), "{reason}");
            drop(framed);
            serve.await.unwrap().unwrap();
            assert_eq!(state.next_session_id.load(Ordering::Relaxed), 1, "{name}");
            assert_eq!(state.active_sessions.load(Ordering::Relaxed), 0, "{name}");
            assert_eq!(state.workspace_manager.loaded_count().await, 0, "{name}");
            assert!(!server_root.exists(), "{name}");
        }
    }

    #[test]
    fn native_position_coordinates_are_checked_before_one_based_conversion() {
        assert_eq!(
            one_based_position(Some(&serde_json::json!({ "line": 0, "character": 0 }))),
            Ok((1, 1))
        );
        assert_eq!(
            one_based_position(Some(&serde_json::json!({
                "line": 4_294_967_294u64,
                "character": 4_294_967_294u64,
            }))),
            Ok((u32::MAX, u32::MAX))
        );

        for malformed in [
            serde_json::json!({}),
            serde_json::json!({ "line": -1, "character": 0 }),
            serde_json::json!({ "line": 0.5, "character": 0 }),
            serde_json::json!({ "line": "0", "character": 0 }),
            serde_json::json!({ "line": null, "character": 0 }),
            serde_json::json!({ "line": 4_294_967_295u64, "character": 0 }),
        ] {
            assert!(one_based_position(Some(&malformed)).is_err(), "{malformed}");
        }
    }

    #[test]
    fn native_methods_validate_positions_without_breaking_positionless_requests() {
        let point = serde_json::json!({ "line": 0, "character": 0 });
        assert!(
            native_position_params(
                Some("textDocument/hover"),
                Some(&serde_json::json!({ "position": point }))
            )
            .is_ok()
        );
        assert!(
            native_position_params(Some("textDocument/hover"), Some(&serde_json::json!({})))
                .is_err()
        );
        assert!(
            native_position_params(
                Some("callHierarchy/incomingCalls"),
                Some(&serde_json::json!({
                    "item": { "selectionRange": { "start": { "line": -1, "character": 0 } } }
                }))
            )
            .is_err()
        );
        assert!(
            native_position_params(
                Some("prodCode/assists"),
                Some(&serde_json::json!({
                    "range": { "start": { "line": 0, "character": 0 } }
                }))
            )
            .is_ok()
        );
        assert!(
            native_position_params(
                Some("prodCode/assists"),
                Some(&serde_json::json!({
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 0, "character": "bad" }
                    }
                }))
            )
            .is_err()
        );
        assert!(
            native_position_params(
                Some("prodCode/structuralReplace"),
                Some(&serde_json::json!({}))
            )
            .is_ok()
        );
        assert!(
            native_position_params(
                Some("prodCode/structuralReplace"),
                Some(&serde_json::json!({ "position": null }))
            )
            .is_err()
        );
        assert!(
            native_position_params(
                Some("textDocument/documentSymbol"),
                Some(&serde_json::json!({}))
            )
            .is_ok()
        );
        assert!(
            native_position_params(Some("workspace/symbol"), Some(&serde_json::json!({}))).is_ok()
        );
        assert!(
            native_position_params(
                Some("textDocument/diagnostic"),
                Some(&serde_json::json!({}))
            )
            .is_ok()
        );
    }

    fn peer(addr: &str, platform: &str, engines: &[&str], load_per_cpu: f64) -> PeerInfo {
        let cpus = 8usize;
        PeerInfo {
            addr: addr.to_string(),
            status: StatusResponse {
                server_pid: 1,
                uptime_seconds: 1,
                active_sessions: 0,
                loaded_workspaces: 0,
                detected_engines: engines.iter().map(|e| e.to_string()).collect(),
                memory_rss_bytes: None,
                total_queries: 0,
                active_queries: 0,
                load_average_millis: Some((load_per_cpu * cpus as f64 * 1000.0) as u32),
                cpu_count: Some(cpus),
                platform: Some(platform.to_string()),
                running_commands: Vec::new(),
                host: Default::default(),
                version: None,
                git_commit: None,
            },
            workspaces: Vec::new(),
            last_seen_secs: 0,
            alive: true,
        }
    }

    /// A node past 85% of its memory or under 10% of its disk gets no new workspace while
    /// another capable node has room, however quiet it is, and gives up an idle one it holds;
    /// one with a session stays, and when every node is short the quietest still takes it
    /// (#396).
    #[test]
    fn a_node_short_of_memory_or_disk_takes_no_new_workspace() {
        let gib = 1 << 30;
        let short_of_disk = HostResources {
            memory_available_bytes: Some(50 * gib),
            memory_total_bytes: Some(100 * gib),
            storage_free_millis: Some(30),
        };
        let short_of_memory = HostResources {
            memory_available_bytes: Some(5 * gib),
            storage_free_millis: Some(500),
            ..short_of_disk.clone()
        };
        let roomy = HostResources {
            storage_free_millis: Some(500),
            ..short_of_disk.clone()
        };
        let mut quiet_but_full = peer("full:9400", "linux x86_64", &["rust (ra_ap_ide)"], 0.05);
        quiet_but_full.status.host = short_of_disk;
        let mut busier = peer("busy:9400", "linux x86_64", &["rust (ra_ap_ide)"], 0.6);
        busier.status.host = roomy;
        let view = ClusterResponse {
            this_node: "full:9400".to_string(),
            nodes: vec![quiet_but_full, busier],
        };
        let place = |view: &ClusterResponse| {
            place_in(
                &PlaceRequest {
                    workspace_name: "subject".to_string(),
                    engine: Some("rust".to_string()),
                    os: None,
                    rebalance_active: false,
                },
                view.clone(),
            )
        };

        let answer = place(&view);
        assert_eq!(
            answer.node.as_deref(),
            Some("busy:9400"),
            "{}",
            answer.reason
        );
        assert!(
            answer
                .reason
                .contains("passed over full:9400 (disk 3% free)"),
            "{}",
            answer.reason
        );

        let mut held = view.clone();
        held.nodes[0].workspaces.push(LoadedWorkspaceInfo {
            name: "subject".to_string(),
            engine: "rust".to_string(),
            sessions: 0,
        });
        let answer = place(&held);
        assert_eq!(
            answer.node.as_deref(),
            Some("busy:9400"),
            "{}",
            answer.reason
        );
        assert!(
            answer
                .reason
                .starts_with("moved from full:9400 (disk 3% free, idle)"),
            "{}",
            answer.reason
        );

        held.nodes[0].workspaces[0].sessions = 1;
        assert_eq!(
            place(&held).node.as_deref(),
            Some("full:9400"),
            "a workspace in use is not moved"
        );

        let rebalance_answer = place_in(
            &PlaceRequest {
                workspace_name: "subject".to_string(),
                engine: Some("rust".to_string()),
                os: None,
                rebalance_active: true,
            },
            held.clone(),
        );
        assert_eq!(
            rebalance_answer.node.as_deref(),
            Some("busy:9400"),
            "rebalance_active moves an active workspace off an overloaded node"
        );
        assert!(
            rebalance_answer.reason.contains("active, 1 sessions"),
            "reason explains session state: {}",
            rebalance_answer.reason
        );

        let mut all_short = view.clone();
        all_short.nodes[1].status.host = short_of_memory;
        let answer = place(&all_short);
        assert_eq!(
            answer.node.as_deref(),
            Some("full:9400"),
            "{}",
            answer.reason
        );
        assert!(
            answer.reason.contains("it is short too (disk 3% free)"),
            "{}",
            answer.reason
        );

        // A gateway too old to report its host is not taken for one that is short.
        let mut old = view.clone();
        old.nodes[1].status.host = HostResources::default();
        assert_eq!(place(&old).node.as_deref(), Some("busy:9400"));
    }

    #[test]
    fn congested_node_rebalances_active_workspace_when_requested() {
        let gib = 1 << 30;
        let roomy = HostResources {
            memory_available_bytes: Some(50 * gib),
            memory_total_bytes: Some(100 * gib),
            storage_free_millis: Some(500),
        };
        let mut congested = peer("congested:9400", "linux x86_64", &["rust (ra_ap_ide)"], 1.5);
        congested.status.host = roomy.clone();
        congested.workspaces.push(LoadedWorkspaceInfo {
            name: "repo".to_string(),
            engine: "rust".to_string(),
            sessions: 2,
        });
        let mut quiet = peer("quiet:9400", "linux x86_64", &["rust (ra_ap_ide)"], 0.2);
        quiet.status.host = roomy;
        let view = ClusterResponse {
            this_node: "congested:9400".to_string(),
            nodes: vec![congested, quiet],
        };

        // Without rebalance_active, active sessions are not moved:
        let normal = place_in(
            &PlaceRequest {
                workspace_name: "repo".to_string(),
                engine: Some("rust".to_string()),
                os: None,
                rebalance_active: false,
            },
            view.clone(),
        );
        assert_eq!(normal.node.as_deref(), Some("congested:9400"));
        assert!(normal.reason.contains("already loaded"));

        // With rebalance_active, active sessions are moved to the quieter node:
        let rebalanced = place_in(
            &PlaceRequest {
                workspace_name: "repo".to_string(),
                engine: Some("rust".to_string()),
                os: None,
                rebalance_active: true,
            },
            view,
        );
        assert_eq!(rebalanced.node.as_deref(), Some("quiet:9400"));
        assert!(rebalanced.reason.contains("rebalanced from congested:9400"));
        assert!(rebalanced.reason.contains("active, 2 sessions"));
    }

    /// Idle engines go after `--idle-evict-secs`, or after five minutes while memory is short,
    /// even when eviction is switched off (#396).
    #[test]
    fn a_host_short_of_memory_unloads_idle_engines_sooner() {
        assert_eq!(evict_after(1800, false), Some(Duration::from_secs(1800)));
        assert_eq!(evict_after(0, false), None);
        assert_eq!(evict_after(1800, true), Some(Duration::from_secs(300)));
        assert_eq!(evict_after(120, true), Some(Duration::from_secs(120)));
        assert_eq!(evict_after(0, true), Some(Duration::from_secs(300)));
    }

    /// A handshake that needs a new engine on a host without the memory for it is refused with
    /// capacity as the reason and a way forward, and leaves no session counted (#433).
    #[tokio::test]
    async fn a_handshake_without_memory_for_its_engine_is_refused_for_capacity() {
        const GIB: u64 = 1 << 30;
        let storage = tempfile::tempdir().expect("tempdir");
        let mut state = ServerState::new(storage.path().join("workspaces"));
        state.workspace_manager = Arc::new(WorkspaceManager::with_admission(Arc::new(
            admission::Admission::with_probe(
                admission::scripted_probe(vec![(10 * GIB, 100 * GIB)]),
                2048,
                admission::LOAD_SETTLE,
            ),
        )));
        let client_root = "/home/dev/app";
        let server_root =
            workspace::resolve_server_workspace(&state.storage_root, client_root, None);
        std::fs::create_dir_all(&server_root).unwrap();
        let state = Arc::new(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let serving = Arc::clone(&state);
        let serve = tokio::spawn(async move {
            let (socket, peer) = listener.accept().await.unwrap();
            handle_client(socket, peer, serving).await
        });
        let stream = TcpStream::connect(addr).await.unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        framed
            .send(WireMessage::HandshakeRequest(
                prod_code_protocol::HandshakeRequest {
                    protocol_version: PROTOCOL_VERSION,
                    supported_versions: Some(vec![PROTOCOL_VERSION]),
                    capabilities: None,
                    client_name: "test".to_string(),
                    client_pid: 1,
                    auth_token: None,
                    client_workspace_root: client_root.to_string(),
                    preferred_engine: None,
                    base_workspace_name: None,
                    engine_subpath: None,
                    client_agent: None,
                    client_host: None,
                    purpose: None,
                    redirect_count: 0,
                },
            ))
            .await
            .unwrap();
        let Some(Ok(WireMessage::Disconnect { reason })) = framed.next().await else {
            panic!("the handshake was not refused");
        };
        assert!(reason.starts_with("capacity: "), "{reason}");
        assert!(reason.contains("memory 90% used"), "{reason}");
        assert!(reason.contains("Retry in a few minutes"), "{reason}");
        assert!(reason.contains("another node"), "{reason}");
        serve.await.unwrap().unwrap();
        assert_eq!(state.active_sessions.load(Ordering::Relaxed), 0);
        assert_eq!(state.workspace_manager.loaded_count().await, 0);
    }

    /// The first answer `state` gives a connection that opens with `token`, if any, and asks for
    /// the status.
    async fn status_answer(state: Arc<ServerState>, token: Option<&str>) -> WireMessage {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let serve = tokio::spawn(async move {
            let (socket, peer) = listener.accept().await.unwrap();
            handle_client(socket, peer, state).await
        });
        let stream = prod_code_protocol::transport::connect_with(addr, token)
            .await
            .unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        framed.send(WireMessage::StatusRequest).await.unwrap();
        let answer = framed.next().await.unwrap().unwrap();
        drop(framed);
        let _ = serve.await;
        answer
    }

    /// A gateway with a token closes every connection that does not open with it and says
    /// why, and serves one that does; a gateway without one ignores a token it is sent (#402).
    #[tokio::test]
    async fn a_gateway_with_a_token_serves_only_connections_that_open_with_it() {
        let storage = tempfile::tempdir().unwrap();
        let mut guarded = ServerState::new(storage.path().join("guarded"));
        guarded.auth_token = Some("s3cret".to_string());
        let guarded = Arc::new(guarded);
        for token in [None, Some("wrong"), Some("s3cre")] {
            match status_answer(Arc::clone(&guarded), token).await {
                WireMessage::Disconnect { reason } => {
                    assert!(reason.contains("PROD_CODE_AUTH_TOKEN"), "{reason}")
                }
                other => panic!("served with {token:?}: {other:?}"),
            }
        }
        assert!(matches!(
            status_answer(Arc::clone(&guarded), Some("s3cret")).await,
            WireMessage::StatusResponse(_)
        ));

        let open = Arc::new(ServerState::new(storage.path().join("open")));
        assert!(matches!(
            status_answer(Arc::clone(&open), Some("anything")).await,
            WireMessage::StatusResponse(_)
        ));
        assert!(matches!(
            status_answer(open, None).await,
            WireMessage::StatusResponse(_)
        ));
    }

    /// An editor is offered what the language server on the node offers, but always asked for
    /// whole documents, which is what the gateway hands on; the Rust engine offers what it
    /// answers (#310).
    #[test]
    fn an_editor_gets_the_servers_capabilities_and_sends_whole_documents() {
        let gopls = serde_json::json!({
            "textDocumentSync": { "openClose": true, "change": 2, "save": {} },
            "completionProvider": { "triggerCharacters": ["."] },
            "hoverProvider": true
        });
        let caps = editor_capabilities(Some(gopls), false);
        assert_eq!(
            caps["textDocumentSync"],
            serde_json::json!({ "openClose": true, "change": 1, "save": {} })
        );
        assert_eq!(
            caps["completionProvider"]["triggerCharacters"],
            serde_json::json!(["."])
        );
        assert_eq!(caps["hoverProvider"], true);

        let numeric =
            editor_capabilities(Some(serde_json::json!({ "textDocumentSync": 2 })), false);
        assert_eq!(
            numeric["textDocumentSync"],
            serde_json::json!({ "openClose": true, "change": 1 })
        );

        let rust = editor_capabilities(None, true);
        assert_eq!(rust["renameProvider"], true);
        assert_eq!(rust["callHierarchyProvider"], true);
        assert_eq!(rust["completionProvider"]["resolveProvider"], true);
        assert_eq!(rust["codeActionProvider"]["resolveProvider"], true);
        assert_eq!(rust["textDocumentSync"]["change"], 1);
        // Every method advertised for Rust is one the engine answers.
        for (capability, method) in [
            ("completionProvider", "textDocument/completion"),
            ("signatureHelpProvider", "textDocument/signatureHelp"),
            ("inlayHintProvider", "textDocument/inlayHint"),
            (
                "documentHighlightProvider",
                "textDocument/documentHighlight",
            ),
            ("codeActionProvider", "textDocument/codeAction"),
            ("documentFormattingProvider", "textDocument/formatting"),
        ] {
            assert!(rust.get(capability).is_some(), "{capability}");
            assert!(
                prod_code_engine_rust::editor::EDITOR_METHODS.contains(&method),
                "{method}"
            );
        }

        assert_eq!(
            editor_capabilities(None, false),
            serde_json::json!({ "textDocumentSync": { "openClose": true, "change": 1 } })
        );
    }

    /// A macOS node is a developer's Mac: it takes work that needs macOS, or that no other live
    /// node serves, and nothing else, however quiet it is (#308).
    #[test]
    fn a_macos_node_takes_only_what_needs_macos_or_what_nothing_else_serves() {
        let view = ClusterResponse {
            this_node: "linux:9400".to_string(),
            nodes: vec![
                peer(
                    "linux:9400",
                    "linux x86_64",
                    &["rust (ra_ap_ide)", "go (gopls)"],
                    0.9,
                ),
                peer(
                    "mac:9400",
                    "macos aarch64",
                    &["swift (sourcekit-lsp)", "go (gopls)"],
                    0.01,
                ),
            ],
        };
        let place = |view: &ClusterResponse, engine: Option<&str>, os: Option<&str>| {
            place_in(
                &PlaceRequest {
                    workspace_name: "subject".to_string(),
                    engine: engine.map(str::to_string),
                    os: os.map(str::to_string),
                    rebalance_active: false,
                },
                view.clone(),
            )
            .node
        };
        assert_eq!(
            place(&view, Some("go"), None).as_deref(),
            Some("linux:9400"),
            "plain Go stays on Linux though the Mac is far quieter"
        );
        assert_eq!(
            place(&view, None, None).as_deref(),
            Some("linux:9400"),
            "so does a workspace whose engine is unknown"
        );
        assert_eq!(
            place(&view, Some("go"), Some("macos")).as_deref(),
            Some("mac:9400"),
            "Go with macOS-only cgo goes to the Mac"
        );
        assert_eq!(
            place(&view, Some("swift"), None).as_deref(),
            Some("mac:9400"),
            "only the Mac serves Swift"
        );

        // A workspace already on the Mac that does not need macOS moves to Linux.
        let mut held = view.clone();
        held.nodes[1].workspaces.push(LoadedWorkspaceInfo {
            name: "subject".to_string(),
            engine: "go".to_string(),
            sessions: 0,
        });
        assert_eq!(
            place(&held, Some("go"), None).as_deref(),
            Some("linux:9400")
        );

        // With the Linux node down, the Mac takes plain Go rather than nothing.
        let mut down = view.clone();
        down.nodes[0].alive = false;
        assert_eq!(place(&down, Some("go"), None).as_deref(), Some("mac:9400"));
    }

    /// A new worktree's copy takes the seed's compiled crates, build-script outputs and
    /// fingerprints with their modification times, and not its incremental caches (#278).
    #[test]
    fn a_seeded_copy_takes_the_build_cache_with_its_times() {
        let dir = tempfile::tempdir().unwrap();
        let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
        let debug = seed.join("target/debug");
        for (rel, text) in [
            ("deps/libdep-1a.rlib", "rlib"),
            ("build/dep-2b/out/generated.rs", "pub const X: u8 = 1;"),
            (".fingerprint/dep-1a/lib-dep", "fingerprint"),
            ("incremental/shop-3c/s-1/query-cache.bin", "incremental"),
        ] {
            let path = debug.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        }
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options()
            .write(true)
            .open(debug.join("deps/libdep-1a.rlib"))
            .unwrap()
            .set_modified(old)
            .unwrap();

        let copied = seed_build_cache_within(&seed, &fresh, roomy()).unwrap();
        assert_eq!(copied, Some(4 + 20 + 11));
        let out = fresh.join("target/debug");
        assert_eq!(
            std::fs::read_to_string(out.join("build/dep-2b/out/generated.rs")).unwrap(),
            "pub const X: u8 = 1;"
        );
        assert!(out.join(".fingerprint/dep-1a/lib-dep").is_file());
        assert!(!out.join("incremental").exists());
        let modified = std::fs::metadata(out.join("deps/libdep-1a.rlib"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            modified, old,
            "cargo compares these times; the copy must keep them"
        );
    }

    /// A new worktree's copy takes the seed's `node_modules` trees, the root's and a workspace
    /// package's, with their symlinks kept as symlinks, and none from under `target` or `.git`;
    /// without room for two of them it takes none (#412).
    #[test]
    fn a_seeded_copy_takes_the_node_modules_trees() {
        let dir = tempfile::tempdir().unwrap();
        let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
        for (rel, text) in [
            (
                "node_modules/zod/index.d.ts",
                "export declare const z: unknown;",
            ),
            ("node_modules/typescript/bin/tsc", "#!/usr/bin/env node"),
            ("node_modules/zod/node_modules/inner/index.js", "nested"),
            ("packages/app/node_modules/left-pad/index.js", "pad"),
            ("target/node_modules/stray.js", "not a package tree"),
            ("src/index.ts", "import { z } from 'zod';"),
        ] {
            let path = seed.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        }
        std::fs::create_dir_all(seed.join("node_modules/.bin")).unwrap();
        std::os::unix::fs::symlink("../typescript/bin/tsc", seed.join("node_modules/.bin/tsc"))
            .unwrap();

        assert_eq!(
            dependency_trees(&seed),
            vec![
                PathBuf::from("node_modules"),
                PathBuf::from("packages/app/node_modules")
            ]
        );
        assert_eq!(
            seed_dependency_trees_within(&seed, &fresh, space(10, 1 << 40)).unwrap(),
            None,
            "no room for two of them"
        );
        assert!(!fresh.join("node_modules").exists());

        let copied = seed_dependency_trees_within(&seed, &fresh, roomy()).unwrap();
        assert_eq!(copied, Some(32 + 19 + 6 + 3));
        assert_eq!(
            std::fs::read_to_string(fresh.join("node_modules/zod/index.d.ts")).unwrap(),
            "export declare const z: unknown;"
        );
        assert!(
            fresh
                .join("node_modules/zod/node_modules/inner/index.js")
                .is_file()
        );
        assert!(
            fresh
                .join("packages/app/node_modules/left-pad/index.js")
                .is_file()
        );
        let link = fresh.join("node_modules/.bin/tsc");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            PathBuf::from("../typescript/bin/tsc")
        );
        assert!(!fresh.join("target").exists());
        assert!(!fresh.join("src").exists(), "sources are copy_tree's");

        let bare = dir.path().join("bare");
        std::fs::create_dir_all(&bare).unwrap();
        assert_eq!(
            seed_dependency_trees_within(&bare, &dir.path().join("fresh2"), roomy()).unwrap(),
            None
        );
    }

    /// A virtual environment reaches the new copy with its symlinks kept (`lib64 -> lib` is not a
    /// second copy of site-packages) and its scripts naming the copy; `copy_tree` leaves it to
    /// the seeding of dependency trees, and keeps a directory symlink as a symlink instead of
    /// walking it (#414).
    #[test]
    fn a_seeded_copy_takes_a_virtualenv_with_its_links_and_its_paths_rewritten() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
        let (venv, copy) = (seed.join(".venv"), fresh.join(".venv"));
        let old = venv.to_str().unwrap().to_string();
        for (rel, text) in [
            ("pyvenv.cfg", "home = /usr/bin\n".to_string()),
            (
                "lib/python3.12/site-packages/pkg/__init__.py",
                "x = 1\n".to_string(),
            ),
            ("bin/pytest", format!("#!{old}/bin/python\nimport pytest\n")),
            (
                "bin/activate",
                format!("VIRTUAL_ENV='{old}'\nexport VIRTUAL_ENV\n"),
            ),
        ] {
            let path = venv.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        }
        std::fs::set_permissions(
            venv.join("bin/pytest"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::os::unix::fs::symlink("lib", venv.join("lib64")).unwrap();
        std::os::unix::fs::symlink("/usr/bin/python3", venv.join("bin/python")).unwrap();
        std::fs::create_dir_all(seed.join("src")).unwrap();
        std::fs::write(seed.join("src/app.py"), "import pkg\n").unwrap();
        std::os::unix::fs::symlink(".", seed.join("src/again")).unwrap();

        assert_eq!(copy_tree(&seed, &fresh).unwrap(), 1, "only src/app.py");
        assert!(!copy.exists(), "the venv is not copy_tree's");
        let again = std::fs::symlink_metadata(fresh.join("src/again")).unwrap();
        assert!(again.file_type().is_symlink());

        assert_eq!(dependency_trees(&seed), vec![PathBuf::from(".venv")]);
        assert!(
            seed_dependency_trees_within(&seed, &fresh, roomy())
                .unwrap()
                .is_some()
        );
        let link = |rel: &str| std::fs::read_link(copy.join(rel)).unwrap();
        assert_eq!(link("lib64"), PathBuf::from("lib"));
        assert_eq!(link("bin/python"), PathBuf::from("/usr/bin/python3"));
        assert!(
            copy.join("lib/python3.12/site-packages/pkg/__init__.py")
                .is_file()
        );
        let new = copy.to_str().unwrap();
        let pytest = std::fs::read_to_string(copy.join("bin/pytest")).unwrap();
        assert_eq!(pytest, format!("#!{new}/bin/python\nimport pytest\n"));
        let mode = std::fs::metadata(copy.join("bin/pytest"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "the script stays executable");
        let activate = std::fs::read_to_string(copy.join("bin/activate")).unwrap();
        assert!(
            activate.contains(&format!("VIRTUAL_ENV='{new}'")),
            "{activate}"
        );
        assert!(!activate.contains(&format!("'{old}'")), "{activate}");
    }

    /// A seeded copy takes the sources and none of the seed's per-node caches: not its CMake
    /// `build/` with the seed's `CMakeCache.txt` and `compile_commands.json`, not clangd's index,
    /// not SwiftPM's `.build` (#416).
    #[test]
    fn a_seeded_copy_takes_no_build_directory_holding_the_seeds_paths() {
        let dir = tempfile::tempdir().unwrap();
        let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
        for rel in [
            "CMakeLists.txt",
            "src/main.cpp",
            "build/CMakeCache.txt",
            "build/compile_commands.json",
            ".cache/clangd/index/main.cpp.1A2B.idx",
            "lib/.build/debug.yaml",
            "tests/__pycache__/test_a.cpython-312.pyc",
        ] {
            let path = seed.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, rel).unwrap();
        }
        assert_eq!(copy_tree(&seed, &fresh).unwrap(), 2);
        assert!(fresh.join("CMakeLists.txt").is_file());
        assert!(fresh.join("src/main.cpp").is_file());
        for cache in ["build", ".cache", "lib/.build", "tests/__pycache__"] {
            assert!(!fresh.join(cache).exists(), "{cache} was copied");
        }
    }

    /// Room for a seed with plenty to spare.
    fn roomy() -> Option<DiskSpace> {
        space(u64::MAX / 4, u64::MAX / 2)
    }

    fn space(free: u64, total: u64) -> Option<DiskSpace> {
        Some(DiskSpace { free, total })
    }

    /// No build cache, not enough room for two of it, or a copy that would leave less than a
    /// fifth of the filesystem free, leaves the new copy without one (#419).
    #[test]
    fn a_seeded_copy_goes_without_a_build_cache_it_has_no_room_for() {
        let dir = tempfile::tempdir().unwrap();
        let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
        assert_eq!(
            seed_build_cache_within(&seed, &fresh, roomy()).unwrap(),
            None
        );
        let deps = seed.join("target/debug/deps");
        std::fs::create_dir_all(&deps).unwrap();
        std::fs::write(deps.join("libbig.rlib"), vec![0u8; 1000]).unwrap();
        assert_eq!(
            seed_build_cache_within(&seed, &fresh, space(1999, 5000)).unwrap(),
            None,
            "not twice its size free"
        );
        assert_eq!(seed_build_cache_within(&seed, &fresh, None).unwrap(), None);
        assert_eq!(
            seed_build_cache_within(&seed, &fresh, space(10_000, 46_000)).unwrap(),
            None,
            "9,000 left of 46,000 is under a fifth"
        );
        assert!(!fresh.join("target").exists());
        assert_eq!(
            seed_build_cache_within(&seed, &fresh, space(10_000, 44_000)).unwrap(),
            Some(1000),
            "9,000 left of 44,000 is more than a fifth"
        );
        assert!(
            disk_space(dir.path()).is_some_and(|s| s.free > 0 && s.total >= s.free),
            "{:?}",
            disk_space(dir.path())
        );
        assert!(disk_space(&dir.path().join("not/yet/created")).is_some());
    }

    /// A running command is in the status for as long as its handler runs, and gone however the
    /// handler returns (#273).
    #[test]
    fn a_running_command_is_listed_until_its_handler_returns() {
        let workspace = Path::new("/srv/workspaces/shop--wt-status-test");
        let mine = |list: &[prod_code_protocol::RunningCommand]| {
            list.iter()
                .filter(|c| c.workspace == "shop--wt-status-test")
                .count()
        };
        let entry = RunningEntry::start(workspace, &["cargo".to_string(), "test".to_string()]);
        let listed = running_commands();
        assert_eq!(mine(&listed), 1);
        let command = listed
            .iter()
            .find(|c| c.workspace == "shop--wt-status-test")
            .unwrap();
        assert_eq!(command.command, "cargo test");
        drop(entry);
        assert_eq!(mine(&running_commands()), 0);
    }

    #[test]
    fn a_compiler_cache_is_shared_across_worktrees_when_the_node_has_ccache() {
        let workspace = Path::new("/srv/workspaces/shop--wt-1a2b");
        assert!(compiler_cache_env(workspace, false).is_empty());
        let env = compiler_cache_env(workspace, true);
        let get = |k: &str| {
            env.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("CCACHE_BASEDIR"), Some("/srv/workspaces/shop--wt-1a2b"));
        assert_eq!(get("CCACHE_NOHASHDIR"), Some("1"));
        assert_eq!(get("CCACHE_SLOPPINESS"), Some("pch_defines,time_macros"));
        assert_eq!(get("CCACHE_PCH_EXTERNAL_CHECKS"), Some("1"));
        assert_eq!(get("CMAKE_C_COMPILER_LAUNCHER"), Some("ccache"));
        assert_eq!(get("CMAKE_CXX_COMPILER_LAUNCHER"), Some("ccache"));
        assert!(on_path("sh"), "sh is on PATH on every node");
        assert!(!on_path("no-such-program-on-any-node"));
    }

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

        let changed = changed_since(root, &before.stamps);
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

    /// A command whose client goes away mid-run leaves the copy exactly as it found it (#262):
    /// the file it rewrote has its old text back, the files it created are gone with the
    /// directory it made, and the file it deleted is there again, executable bit included.
    #[tokio::test]
    async fn a_command_whose_client_leaves_changes_nothing_in_the_copy() {
        let storage = tempfile::tempdir().unwrap();
        let workspace = storage.path().join("restore-ws");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::write(workspace.join("src/a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(workspace.join("src/gone.rs"), "fn gone() {}\n").unwrap();
        std::fs::write(workspace.join("run.sh"), "#!/bin/sh\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                workspace.join("run.sh"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        let before = stamp_tree(&workspace);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let storage_root = storage.path().to_path_buf();
        let metrics_dir = tempfile::tempdir().unwrap();
        let metrics = metrics::Metrics::new(metrics_dir.path().to_path_buf());
        let gateway = tokio::spawn(async move {
            let manager = WorkspaceManager::new();
            let mut framed = Framed::new(AnyStream::from(server), ProdCodeCodec::new());
            let req = ExecRequest {
                client_workspace_root: "/tmp/restore-ws".to_string(),
                base_workspace_name: Some("restore-ws".to_string()),
                command: [
                    "sh",
                    "-c",
                    "printf 'fn a() { formatted }' > src/a.rs; printf new > src/new.rs; \
                     rm src/gone.rs run.sh; mkdir -p src/deep; printf x > src/deep/made.rs; \
                     echo ready; sleep 60",
                ]
                .map(str::to_string)
                .to_vec(),
                env: Vec::new(),
                timeout_secs: 120,
                pull_changes: true,
                subdir: None,
                client_agent: None,
                client_host: None,
            };
            run_exec(&storage_root, &metrics, &manager, &mut framed, req).await
        });

        let mut framed = Framed::new(client, ProdCodeCodec::new());
        let mut output = Vec::new();
        while !String::from_utf8_lossy(&output).contains("ready") {
            match tokio::time::timeout(std::time::Duration::from_secs(30), framed.next()).await {
                Ok(Some(Ok(WireMessage::ExecChunk(chunk)))) => {
                    output.extend(chunk.data.unwrap_or_default())
                }
                other => panic!("no output from the command: {other:?}"),
            }
        }
        assert!(workspace.join("src/new.rs").is_file(), "the command ran");
        drop(framed);

        tokio::time::timeout(std::time::Duration::from_secs(30), gateway)
            .await
            .expect("the command was killed, not left to run out its sleep")
            .unwrap()
            .unwrap();
        assert_eq!(stamp_tree(&workspace), before);
        assert_eq!(
            std::fs::read_to_string(workspace.join("src/a.rs")).unwrap(),
            "fn a() {}\n"
        );
        assert!(!workspace.join("src/deep").exists());
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(workspace.join("run.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        let mut stale = workspace::stale_paths(&workspace);
        stale.sort();
        assert_eq!(stale, vec!["run.sh", "src/a.rs", "src/gone.rs"]);
    }

    /// A file that a client sync delivered while the command ran is the checkout's text and is
    /// left alone; one the command changed again after it arrived is removed and reported stale;
    /// one only the command changed gets its old bytes back (#262).
    #[test]
    fn a_restore_keeps_what_a_sync_delivered_during_the_command() {
        let storage = tempfile::tempdir().unwrap();
        let root = storage.path();
        for name in ["cmd.txt", "synced.txt", "both.txt"] {
            std::fs::write(root.join(name), "old\n").unwrap();
        }
        let before = snapshot_tree_within(root, RESTORE_MAX_FILE, RESTORE_BUDGET);
        std::fs::write(root.join("cmd.txt"), "the command's\n").unwrap();
        std::fs::write(root.join("synced.txt"), "the client's\n").unwrap();
        std::fs::write(root.join("both.txt"), "the command's, after the sync\n").unwrap();
        let synced = std::collections::HashMap::from([
            (
                "synced.txt".to_string(),
                Some(content_hash(b"the client's\n")),
            ),
            (
                "both.txt".to_string(),
                Some(content_hash(b"the client's\n")),
            ),
        ]);

        let restored = restore_tree(root, &before, &synced);

        assert_eq!(
            std::fs::read_to_string(root.join("cmd.txt")).unwrap(),
            "old\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("synced.txt")).unwrap(),
            "the client's\n"
        );
        assert!(!root.join("both.txt").exists());
        assert_eq!(
            restored.stale,
            vec!["both.txt".to_string(), "cmd.txt".to_string()]
        );
        let mut files: Vec<&str> = restored
            .files
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();
        files.sort();
        assert_eq!(files, vec!["both.txt", "cmd.txt"]);
    }

    #[test]
    fn a_sync_is_remembered_per_workspace_from_the_time_it_lands() {
        let one = std::path::Path::new("/nonexistent/sync-log-one");
        let other = std::path::Path::new("/nonexistent/sync-log-other");
        let start = Instant::now();
        workspace::record_synced(
            one,
            &[("a.rs".to_string(), Some(7)), ("gone.rs".to_string(), None)],
        );
        let synced = workspace::synced_since(one, start);
        assert_eq!(synced.get("a.rs"), Some(&Some(7)));
        assert_eq!(synced.get("gone.rs"), Some(&None));
        assert!(workspace::synced_since(other, start).is_empty());
        assert!(workspace::synced_since(one, Instant::now()).is_empty());
        workspace::record_synced(one, &[]);
    }

    /// A file whose old bytes did not fit the snapshot's limits cannot be put back: it leaves the
    /// copy and is reported stale by every sync answer until the client has sent it (#262).
    #[tokio::test]
    async fn a_file_past_the_snapshot_limits_is_removed_and_reported_stale() {
        let storage = tempfile::tempdir().unwrap();
        let root = storage.path().join("ws");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "aaaa").unwrap();
        std::fs::write(root.join("src/b.rs"), "bbbb").unwrap();
        std::fs::write(root.join("src/big.rs"), "0123456789").unwrap();
        // Files are kept in path order: a.rs fits, b.rs is over the budget a.rs leaves, and
        // big.rs is over the per-file limit.
        let before = snapshot_tree_within(&root, 8, 6);
        assert_eq!(before.kept.keys().collect::<Vec<_>>(), ["src/a.rs"]);

        std::fs::write(root.join("src/a.rs"), "changed").unwrap();
        std::fs::write(root.join("src/b.rs"), "changed").unwrap();
        std::fs::remove_file(root.join("src/big.rs")).unwrap();
        let manager = WorkspaceManager::new();
        let restored =
            restore_after_lost_client(&manager, &root, Arc::new(before), Instant::now()).await;
        assert_eq!(restored, 1);
        assert_eq!(
            std::fs::read_to_string(root.join("src/a.rs")).unwrap(),
            "aaaa"
        );
        assert!(
            !root.join("src/b.rs").exists(),
            "a changed file not kept leaves"
        );
        assert_eq!(
            workspace::stale_paths(&root),
            [
                "src/a.rs".to_string(),
                "src/b.rs".to_string(),
                "src/big.rs".to_string()
            ]
        );

        // The next sync answers with what it still lacks until the client has sent both.
        let sync = |files: Vec<FileDelta>| SyncRequest {
            client_workspace_root: "/tmp/ws".to_string(),
            files,
            clean_others: false,
            base_workspace_name: Some("ws".to_string()),
        };
        let first = apply_sync(
            storage.path(),
            &manager,
            sync(vec![
                FileDelta {
                    relative_path: "src/a.rs".to_string(),
                    content: Some(b"aaaa".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: "src/b.rs".to_string(),
                    content: Some(b"bbbb".to_vec()),
                    is_executable: false,
                },
            ]),
        )
        .await;
        assert_eq!(first.stale_paths, ["src/big.rs".to_string()]);
        let second = apply_sync(
            storage.path(),
            &manager,
            sync(vec![FileDelta {
                relative_path: "src/big.rs".to_string(),
                content: None,
                is_executable: false,
            }]),
        )
        .await;
        assert!(second.stale_paths.is_empty());
        assert!(!root.join(workspace::STALE_MARKER).exists());
    }

    /// A file the copy cannot take keeps its old text, is not counted, and comes back stale until
    /// the client has sent it again. A read-only directory stands in for a full disk, where
    /// `fs::write` truncated the file and reported nothing (#385).
    #[tokio::test]
    async fn a_sync_write_that_fails_keeps_the_old_text_and_asks_for_it_again() {
        use std::os::unix::fs::PermissionsExt;
        let storage = tempfile::tempdir().unwrap();
        let manager = WorkspaceManager::new();
        let sync = |text: &str| SyncRequest {
            client_workspace_root: "/tmp/ws".to_string(),
            files: vec![FileDelta {
                relative_path: "src/lib.rs".to_string(),
                content: Some(text.as_bytes().to_vec()),
                is_executable: false,
            }],
            clean_others: false,
            base_workspace_name: Some("ws".to_string()),
        };
        apply_sync(storage.path(), &manager, sync("fn old() {}")).await;
        let root = storage.path().join("ws");
        let src = root.join("src");
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o555)).unwrap();
        let refused = apply_sync(storage.path(), &manager, sync("fn new() {}")).await;
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            std::fs::read_to_string(src.join("lib.rs")).unwrap(),
            "fn old() {}"
        );
        assert_eq!(refused.files_updated, 0);
        assert_eq!(refused.stale_paths, ["src/lib.rs".to_string()]);
        assert_eq!(workspace::stale_paths(&root), ["src/lib.rs".to_string()]);
        assert!(
            std::fs::read_dir(&src).unwrap().count() == 1,
            "no temporary file is left behind"
        );

        let again = apply_sync(storage.path(), &manager, sync("fn new() {}")).await;
        assert_eq!(again.files_updated, 1);
        assert!(again.stale_paths.is_empty());
        assert_eq!(
            std::fs::read_to_string(src.join("lib.rs")).unwrap(),
            "fn new() {}"
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

        let java_temp = tempfile::tempdir().unwrap();
        std::fs::write(java_temp.path().join("pom.xml"), "").unwrap();
        assert_eq!(detect_engine(java_temp.path()), EngineKind::Java);

        let kt_temp = tempfile::tempdir().unwrap();
        std::fs::write(kt_temp.path().join("build.gradle.kts"), "").unwrap();
        assert_eq!(detect_engine(kt_temp.path()), EngineKind::Kotlin);

        let cs_temp = tempfile::tempdir().unwrap();
        std::fs::write(cs_temp.path().join("App.csproj"), "").unwrap();
        assert_eq!(detect_engine(cs_temp.path()), EngineKind::Csharp);

        let php_temp = tempfile::tempdir().unwrap();
        std::fs::write(php_temp.path().join("composer.json"), "").unwrap();
        assert_eq!(detect_engine(php_temp.path()), EngineKind::Php);

        let rb_temp = tempfile::tempdir().unwrap();
        std::fs::write(rb_temp.path().join("Gemfile"), "").unwrap();
        assert_eq!(detect_engine(rb_temp.path()), EngineKind::Ruby);
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

    #[cfg(unix)]
    #[tokio::test]
    async fn sync_rejects_absolute_parent_and_symlink_paths() {
        use std::os::unix::fs::symlink;

        let storage = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let manager = WorkspaceManager::new();
        let root = storage.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        symlink(outside.path(), root.join("linked")).unwrap();

        let victim = outside.path().join("victim.txt");
        std::fs::write(&victim, b"keep").unwrap();
        let absolute_write = outside.path().join("absolute-escape.txt");
        let parent_write = outside.path().join("parent-escape.txt");
        let symlink_write = outside.path().join("symlink-escape.txt");
        let response = apply_sync(
            storage.path(),
            &manager,
            SyncRequest {
                client_workspace_root: "/tmp/ws".to_string(),
                files: vec![
                    FileDelta {
                        relative_path: "../parent-escape.txt".to_string(),
                        content: Some(b"parent".to_vec()),
                        is_executable: false,
                    },
                    FileDelta {
                        relative_path: absolute_write.to_string_lossy().into_owned(),
                        content: Some(b"absolute".to_vec()),
                        is_executable: false,
                    },
                    FileDelta {
                        relative_path: "linked/symlink-escape.txt".to_string(),
                        content: Some(b"symlink".to_vec()),
                        is_executable: false,
                    },
                    FileDelta {
                        relative_path: "../victim.txt".to_string(),
                        content: None,
                        is_executable: false,
                    },
                ],
                clean_others: false,
                base_workspace_name: Some("ws".to_string()),
            },
        )
        .await;

        assert_eq!(response.files_updated, 0);
        assert_eq!(response.files_deleted, 0);
        for rejected in [
            "../parent-escape.txt",
            absolute_write.to_str().unwrap(),
            "linked/symlink-escape.txt",
            "../victim.txt",
        ] {
            assert!(
                response.stale_paths.iter().any(|path| path == rejected),
                "invalid path should be retried: {rejected:?}; stale paths: {:?}",
                response.stale_paths
            );
        }
        assert!(!parent_write.exists());
        assert!(!absolute_write.exists());
        assert!(!symlink_write.exists());
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
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

    /// A refactoring's rewrites are read at the paths the files had before it, and LSP applies
    /// `documentChanges` in order, so they go out before the moves. Sent after them, the rewrite
    /// of `a.rs` would land on the file `c.rs` was just moved to. Applied by the client, a module
    /// rename (`foo.rs` and `foo/`, with a file created inside it) lands whole.
    #[test]
    fn a_refactoring_is_serialized_with_its_rewrites_before_its_moves() {
        use prod_code_engine_rust::{FileMove, RefactorOutcome, RewrittenFile};
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("src/lib.rs", "mod foo;\nmod a;\nmod c;\n");
        write("src/foo.rs", "mod inner;\npub use inner::f;\n");
        write("src/foo/inner.rs", "pub fn f() -> u8 { crate::foo::X }\n");
        write("src/a.rs", "pub const A: u8 = 1;\n");
        write("src/c.rs", "pub const C: u8 = 3;\n");
        let rewrite = |rel: &str, new_text: &str| RewrittenFile {
            path: root.join(rel),
            new_text: new_text.to_string(),
            edits: 1,
            old_line_count: std::fs::read_to_string(root.join(rel))
                .unwrap()
                .lines()
                .count() as u32,
        };
        let moved = |from: &str, to: &str| FileMove {
            from: root.join(from),
            to: root.join(to),
        };
        let outcome = RefactorOutcome {
            files: vec![
                rewrite("src/lib.rs", "mod bar;\nmod b;\nmod a;\n"),
                rewrite("src/foo.rs", "mod inner;\nmod extra;\npub use inner::f;\n"),
                rewrite("src/foo/inner.rs", "pub fn f() -> u8 { crate::bar::X }\n"),
                rewrite("src/a.rs", "pub const B: u8 = 1;\n"),
                rewrite("src/c.rs", "pub const A: u8 = 3;\n"),
            ],
            created: vec![RewrittenFile {
                path: root.join("src/foo/extra.rs"),
                new_text: "pub fn extra() {}\n".to_string(),
                edits: 1,
                old_line_count: 0,
            }],
            moves: vec![
                moved("src/foo.rs", "src/bar.rs"),
                moved("src/foo", "src/bar"),
                moved("src/a.rs", "src/b.rs"),
                moved("src/c.rs", "src/a.rs"),
            ],
        };
        let edit = super::workspace_edit_json(&outcome);
        let kinds: Vec<&str> = edit["documentChanges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|change| change["kind"].as_str().unwrap_or("edit"))
            .collect();
        assert_eq!(
            kinds,
            [
                "create", "edit", "edit", "edit", "edit", "edit", "edit", "rename", "rename",
                "rename", "rename"
            ]
        );
        prod_code_mcp::refactor::apply_workspace_edit(&root, &edit).unwrap();
        let read = |rel: &str| std::fs::read_to_string(root.join(rel)).ok();
        assert_eq!(
            read("src/lib.rs").as_deref(),
            Some("mod bar;\nmod b;\nmod a;\n")
        );
        assert_eq!(
            read("src/bar.rs").as_deref(),
            Some("mod inner;\nmod extra;\npub use inner::f;\n")
        );
        assert_eq!(
            read("src/bar/inner.rs").as_deref(),
            Some("pub fn f() -> u8 { crate::bar::X }\n")
        );
        assert_eq!(
            read("src/bar/extra.rs").as_deref(),
            Some("pub fn extra() {}\n")
        );
        assert_eq!(read("src/b.rs").as_deref(), Some("pub const B: u8 = 1;\n"));
        assert_eq!(read("src/a.rs").as_deref(), Some("pub const A: u8 = 3;\n"));
        for gone in ["src/foo.rs", "src/foo", "src/c.rs"] {
            assert!(!root.join(gone).exists(), "{gone} was moved away");
        }
        prod_code_mcp::sync::clear_sync_cache(&root);
    }

    /// The same through rust-analyzer: renaming the module `foo`, kept in `foo.rs` with its
    /// submodule in `foo/`, moves both and rewrites every use, the one inside `foo/` included,
    /// and the client lands all of it.
    #[test]
    fn a_module_rename_by_the_analyzer_lands_whole_in_the_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write(
            "src/lib.rs",
            "pub mod foo;\npub fn g() -> u8 { foo::inner::f() }\n",
        );
        write("src/foo.rs", "pub mod inner;\npub const X: u8 = 1;\n");
        write("src/foo/inner.rs", "pub fn f() -> u8 { crate::foo::X }\n");
        let engine = prod_code_engine_rust::RustEngine::load(&root).unwrap();
        // `foo` in `pub mod foo;` is line 1, column 9.
        let outcome = engine
            .rename(&root.join("src/lib.rs"), 1, 9, "bar")
            .expect("rename query")
            .expect("rename accepted");
        assert_eq!(outcome.moves.len(), 2, "{outcome:?}");
        let edit = super::workspace_edit_json(&outcome);
        prod_code_mcp::refactor::apply_workspace_edit(&root, &edit).unwrap();
        let read = |rel: &str| std::fs::read_to_string(root.join(rel)).ok();
        assert_eq!(
            read("src/lib.rs").as_deref(),
            Some("pub mod bar;\npub fn g() -> u8 { bar::inner::f() }\n")
        );
        assert_eq!(
            read("src/bar.rs").as_deref(),
            Some("pub mod inner;\npub const X: u8 = 1;\n")
        );
        assert_eq!(
            read("src/bar/inner.rs").as_deref(),
            Some("pub fn f() -> u8 { crate::bar::X }\n")
        );
        assert!(!root.join("src/foo.rs").exists() && !root.join("src/foo").exists());
        prod_code_mcp::sync::clear_sync_cache(&root);
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

#[cfg(test)]
mod exec_resilience_tests {
    use super::*;

    #[tokio::test]
    async fn refresh_engines_safely_handles_unloaded_or_failing_updates() {
        let storage = tempfile::tempdir().unwrap();
        let manager = WorkspaceManager::new();
        let ws_dir = storage.path().join("ws");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let files = vec![FileDelta {
            relative_path: "src/lib.rs".to_string(),
            content: Some(b"pub fn dummy() {}\n".to_vec()),
            is_executable: false,
        }];
        refresh_engines(&manager, &ws_dir, &files).await;
    }

    #[test]
    fn test_effective_prune_timeouts() {
        let cli = ServerCli {
            bind: "0.0.0.0:9400".parse().unwrap(),
            socket_path: None,
            storage: PathBuf::from("/tmp/storage"),
            idle_evict_secs: 1800,
            engine_reserve_mib: 0,
            max_concurrent_engine_loads: 0,
            prune_worktree_secs: 3600,
            prune_worktree_days: None,
            prune_workspace_secs: 86400,
            prune_workspace_days: None,
            prune_below_free_percent: 15,
            engines: vec![],
            shadow_dir: None,
            peers: String::new(),
            advertise: None,
            build_cache_ram: false,
            build_cache_dir: None,
        };
        // Defaults: 1 hour (3600s) for worktrees, 24 hours (86400s) for main workspaces
        assert_eq!(cli.effective_prune_worktree_secs(), 3600);
        assert_eq!(cli.effective_prune_workspace_secs(), 86400);

        // Days overrides
        let mut cli_days = cli.clone();
        cli_days.prune_worktree_days = Some(7);
        cli_days.prune_workspace_days = Some(3);
        assert_eq!(cli_days.effective_prune_worktree_secs(), 7 * 86_400);
        assert_eq!(cli_days.effective_prune_workspace_secs(), 3 * 86_400);

        // Disabling with 0
        let mut cli_disabled = cli.clone();
        cli_disabled.prune_worktree_secs = 0;
        cli_disabled.prune_workspace_days = Some(0);
        assert_eq!(cli_disabled.effective_prune_worktree_secs(), 0);
        assert_eq!(cli_disabled.effective_prune_workspace_secs(), 0);
    }

    #[test]
    fn test_prewarm_virtualenv_pycache_safe_on_missing_or_invalid() {
        let temp = tempfile::tempdir().unwrap();
        let empty_venv = temp.path().join("empty_venv");
        std::fs::create_dir_all(&empty_venv).unwrap();
        // Missing python binary -> Ok(0)
        let res = prewarm_virtualenv_pycache(&empty_venv);
        assert_eq!(res.unwrap(), 0);

        // Invalid non-executable python file -> Ok(0) without crashing
        let bin_dir = empty_venv.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let py_file = bin_dir.join("python");
        std::fs::write(&py_file, b"not executable").unwrap();
        let res = prewarm_virtualenv_pycache(&empty_venv);
        assert_eq!(res.unwrap(), 0);
    }

    #[test]
    fn test_polyglot_compiler_cache_env_composition() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("my-project");
        std::fs::create_dir_all(&ws).unwrap();

        let ram_target = temp.path().join("ram-target");
        let envs = polyglot_compiler_cache_env(&ws, false, Some(&ram_target));
        assert!(envs.iter().any(|(k, v)| k == "CARGO_TARGET_DIR" && v == ram_target.to_str().unwrap()));
        assert!(envs.iter().any(|(k, _)| k == "SWIFTPM_MODULECACHE_OVERRIDE"));
        assert!(envs.iter().any(|(k, _)| k == "SWIFT_MODULE_CACHE_PATH"));
        assert!(envs.iter().any(|(k, _)| k == "CLANG_MODULE_CACHE_PATH"));

        let ccache_envs = polyglot_compiler_cache_env(&ws, true, None);
        assert!(ccache_envs.iter().any(|(k, v)| k == "CCACHE_BASEDIR" && v == ws.to_str().unwrap()));
        assert!(ccache_envs.iter().any(|(k, v)| k == "CCACHE_NOHASHDIR" && v == "1"));
    }

    #[test]
    fn test_resolve_ram_build_cache_workspace_isolation() {
        let temp = tempfile::tempdir().unwrap();
        let ws1 = temp.path().join("repo-a");
        let ws2 = temp.path().join("repo-b");
        std::fs::create_dir_all(&ws1).unwrap();
        std::fs::create_dir_all(&ws2).unwrap();

        let custom_ram = temp.path().join("custom-shm");
        // Disabled returns None
        assert!(resolve_ram_build_cache(&ws1, false, Some(&custom_ram)).is_none());

        // Enabled creates isolated target directories
        let target1 = resolve_ram_build_cache(&ws1, true, Some(&custom_ram)).expect("target1");
        let target2 = resolve_ram_build_cache(&ws2, true, Some(&custom_ram)).expect("target2");

        assert!(target1.exists());
        assert!(target2.exists());
        assert_ne!(target1, target2, "workspaces must receive isolated RAM target directories");
        assert!(target1.ends_with("target"));
        assert!(target2.ends_with("target"));
        assert!(target1.parent().unwrap().join(".last_used").exists());
        assert!(target2.parent().unwrap().join(".last_used").exists());
    }

    #[test]
    fn test_sweep_ram_build_caches_removes_old_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let ram_base = temp.path().join("shm");
        let ws_dir = ram_base.join("ws-old");
        let target_dir = ws_dir.join("target");
        std::fs::create_dir_all(&target_dir).unwrap();

        // Fresh dir is not swept (not older than 24h)
        let swept = sweep_ram_build_caches(&ram_base);
        assert_eq!(swept, 0);
        assert!(ws_dir.exists());

        // Active lease prevents sweeping and is recognized as active
        let lease = RamBuildLease::acquire(&target_dir);
        assert!(lease.marker.is_some());
        let marker_path = lease.marker.clone().unwrap();
        assert!(marker_path.exists());
        assert!(is_ram_lease_active(&marker_path));
        let swept_active = sweep_ram_build_caches(&ram_base);
        assert_eq!(swept_active, 0);
        assert!(ws_dir.exists());
        assert!(marker_path.exists());

        // Dropping lease unlinks its marker file
        drop(lease);
        assert!(!marker_path.exists());

        // Stale lease marker (left by crashed process) is detected as inactive and cleaned up
        let stale_lease = ws_dir.join(".active_99999_1");
        std::fs::write(&stale_lease, b"stale").unwrap();
        assert!(stale_lease.exists());
        assert!(!is_ram_lease_active(&stale_lease));
        assert!(!stale_lease.exists(), "stale lease marker must be removed when unowned");

        // Old dir (>24h) with stale lease marker is swept cleanly without being permanently protected
        let stale_old = ws_dir.join(".active_88888_2");
        std::fs::write(&stale_old, b"stale-old").unwrap();
        let last_used = ws_dir.join(".last_used");
        let f = std::fs::File::create(&last_used).unwrap();
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(100_000);
        f.set_modified(past).unwrap();
        drop(f);

        let swept_stale_old = sweep_ram_build_caches(&ram_base);
        assert_eq!(swept_stale_old, 1);
        assert!(!ws_dir.exists(), "stale crash-left marker must not permanently protect cache directory");

        // Non-existent base dir returns 0 safely
        let swept_none = sweep_ram_build_caches(&temp.path().join("does_not_exist"));
        assert_eq!(swept_none, 0);
    }

    #[test]
    fn test_prewarm_virtualenv_pycache_does_not_execute_workspace_binary() {
        let temp = tempfile::tempdir().unwrap();
        let malicious_venv = temp.path().join("malicious_venv");
        let bin_dir = malicious_venv.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let canary = temp.path().join("canary_executed.txt");
        let fake_python = bin_dir.join("python");
        // Script that would create the canary file if executed
        std::fs::write(
            &fake_python,
            format!("#!/bin/sh\ntouch {}\n", canary.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_python, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let _ = prewarm_virtualenv_pycache(&malicious_venv);
        assert!(
            !canary.exists(),
            "prewarm_virtualenv_pycache must never execute untrusted workspace python binary"
        );
    }

    #[test]
    fn test_resolve_ram_build_cache_false_env_does_not_enable() {
        assert!(!is_ram_cache_enabled_with(false, Some("false")));
        assert!(!is_ram_cache_enabled_with(false, Some("0")));
        assert!(!is_ram_cache_enabled_with(false, Some("no")));
        assert!(!is_ram_cache_enabled_with(false, Some("off")));
        assert!(!is_ram_cache_enabled_with(false, None));

        assert!(is_ram_cache_enabled_with(false, Some("true")));
        assert!(is_ram_cache_enabled_with(false, Some("1")));
        assert!(is_ram_cache_enabled_with(false, Some("yes")));
        assert!(is_ram_cache_enabled_with(false, Some("on")));
        assert!(is_ram_cache_enabled_with(true, Some("false")));
        assert!(is_ram_cache_enabled_with(true, None));
    }
}


