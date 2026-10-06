/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;


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
    pub _shadow_root_owner: Option<shadow::ShadowRootOwner>,
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
pub(crate) const AUTH_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// What a connection without the cluster's token is told before it is closed.
pub(crate) const AUTH_REFUSED: &str = "this gateway requires the cluster's connection token: set PROD_CODE_AUTH_TOKEN to it, or PROD_CODE_AUTH_TOKEN_FILE to a file that holds it";


/// A peer's latest heartbeat.
pub struct PeerEntry {
    pub gossip: NodeGossip,
    pub last_seen: Instant,
}

/// How long a silent peer still counts as alive.
pub(crate) const PEER_ALIVE: std::time::Duration = std::time::Duration::from_secs(30);
/// How long a silent peer stays in the cluster before eviction.
pub(crate) const PEER_EVICT: std::time::Duration = std::time::Duration::from_secs(60);
/// Heartbeat period.
pub(crate) const GOSSIP_PERIOD: std::time::Duration = std::time::Duration::from_secs(5);

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
    pub fn advertised_engines(&self) -> Vec<String> {
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

