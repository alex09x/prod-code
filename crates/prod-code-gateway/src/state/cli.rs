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
