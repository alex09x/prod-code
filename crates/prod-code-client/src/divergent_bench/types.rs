/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Minimum number of concurrent simulated workers required by the benchmark contract.
pub const MIN_WORKERS: usize = 10;

/// Suffix appended to the base repository name to form the origin clone / server workspace
/// name, keeping benchmark traffic away from the real shared workspace of the same repository.
pub const BENCH_WORKSPACE_SUFFIX: &str = "-divergent-bench";

pub(crate) const FIXTURE_REPO_NAME: &str = "fixture";
pub(crate) const FIXTURE_CARGO_TOML: &str = "[package]\nname = \"divergent-bench-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n";
pub(crate) const FIXTURE_LIB_RS: &str =
    "pub fn compute_signal(input: i64) -> i64 {\n    input * 2\n}\n";

/// Caps each initial sync and session setup so a silent gateway cannot leave a
/// benchmark worker hanging forever. This is deliberately separate from the 30-second hover
/// response budget, which measures query behavior rather than session setup reliability.
pub(crate) const SESSION_SETUP_TIMEOUT: Duration = Duration::from_secs(300);

/// Language of the base repository, which decides manifest, mutation and symbol conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Go,
}

impl Language {
    pub fn label(&self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Go => "go",
        }
    }

    pub(crate) fn manifest(&self) -> &'static str {
        match self {
            Language::Rust => "Cargo.toml",
            Language::Go => "go.mod",
        }
    }

    pub(crate) fn extension(&self) -> &'static str {
        match self {
            Language::Rust => "rs",
            Language::Go => "go",
        }
    }

    /// Extra parameter appended to the target signature in worktree A.
    pub(crate) fn marker_param(&self) -> &'static str {
        match self {
            Language::Rust => "divergent_marker: i64",
            Language::Go => "divergentMarker int",
        }
    }

    /// Identifier that must appear in worktree A's hover and nowhere else.
    pub fn marker(&self) -> &'static str {
        match self {
            Language::Rust => "divergent_marker",
            Language::Go => "divergentMarker",
        }
    }

    /// Symbol introduced by worktree C's untracked file.
    pub fn untracked_symbol(&self) -> &'static str {
        match self {
            Language::Rust => "divergent_untracked_symbol",
            Language::Go => "DivergentUntrackedSymbol",
        }
    }

    pub(crate) fn untracked_file_name(&self) -> &'static str {
        match self {
            Language::Rust => "divergent_untracked.rs",
            Language::Go => "divergent_untracked.go",
        }
    }

    pub(crate) fn manifest_touch_line(&self) -> &'static str {
        match self {
            Language::Rust => "\n# divergent-bench: dependency manifest touched by worktree B\n",
            Language::Go => "\n// divergent-bench: dependency manifest touched by worktree B\n",
        }
    }
}

/// How the four worktrees map onto server workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum WorkspaceMode {
    /// All worktrees coalesce onto one shared server workspace and one analysis database.
    /// Diagnostic only: production gives every worktree its own workspace.
    Shared,
    /// Each worktree gets a dedicated server workspace and engine instance (production).
    #[default]
    Isolated,
}

impl WorkspaceMode {
    pub fn label(&self) -> &'static str {
        match self {
            WorkspaceMode::Shared => "shared",
            WorkspaceMode::Isolated => "isolated",
        }
    }
}

/// Which of the four diverged workspaces a worktree/query/result belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WorktreeKind {
    Master,
    SignatureChange,
    DependencyChange,
    UntrackedFile,
}

impl WorktreeKind {
    pub fn label(&self) -> &'static str {
        match self {
            WorktreeKind::Master => "master",
            WorktreeKind::SignatureChange => "worktree-a-signature",
            WorktreeKind::DependencyChange => "worktree-b-dependency",
            WorktreeKind::UntrackedFile => "worktree-c-untracked",
        }
    }

    pub(crate) fn dir_name(&self) -> &'static str {
        match self {
            WorktreeKind::Master => "wt-master",
            WorktreeKind::SignatureChange => "wt-signature",
            WorktreeKind::DependencyChange => "wt-dependency",
            WorktreeKind::UntrackedFile => "wt-untracked",
        }
    }

    pub fn all() -> [WorktreeKind; 4] {
        [
            WorktreeKind::Master,
            WorktreeKind::SignatureChange,
            WorktreeKind::DependencyChange,
            WorktreeKind::UntrackedFile,
        ]
    }
}

/// The real symbol discovered in the base repository that every worktree is queried against.
#[derive(Debug, Clone)]
pub struct DivergentTarget {
    pub language: Language,
    /// Path of the file relative to the repository root.
    pub file_rel: PathBuf,
    pub symbol: String,
    /// Zero-based line of the signature.
    pub line: usize,
}

/// One isolated `git worktree`, mutated according to its [`WorktreeKind`], along with the
/// specific file/symbol the benchmark will query against it.
#[derive(Debug, Clone)]
pub struct DivergentWorktree {
    pub kind: WorktreeKind,
    pub root: PathBuf,
    pub query_file: PathBuf,
    pub symbol: String,
    /// Base workspace name handed to the gateway on handshake and sync.
    pub workspace_name: String,
}

/// The prepared origin repository plus its four diverged worktrees.
pub struct DivergenceSetup {
    pub origin: PathBuf,
    pub workspace_name: String,
    pub target: DivergentTarget,
    pub worktrees: Vec<DivergentWorktree>,
}

/// Configuration for a single divergent-benchmark run.
#[derive(Debug, Clone)]
pub struct DivergentBenchConfig {
    /// Address of the prod-code remote gateway to hammer with LSP queries.
    pub remote: SocketAddr,
    /// Base git repository to fork worktrees from (a checkout of a real Rust or Go repository).
    /// When `None`, a disposable scratch repository is created instead.
    pub base_repo: Option<PathBuf>,
    /// Scratch directory to materialize the origin clone and worktrees in.
    /// When `None`, a fresh temp directory is created and removed on completion.
    pub workdir: Option<PathBuf>,
    /// Number of concurrent simulated workers. Must be >= [`MIN_WORKERS`].
    pub workers: usize,
    /// Number of queries each worker issues against its assigned worktree.
    pub queries_per_worker: usize,
    /// Keep the generated scratch worktrees on disk after the run for inspection.
    pub keep_workdir: bool,
    /// Shared or isolated server workspaces.
    pub mode: WorkspaceMode,
    /// One gateway session per worker (sync + handshake + initialize once, then only
    /// didOpen/hover/didClose per query), the way a long-lived MCP agent process behaves.
    /// Off: a fresh connection with pre-flight sync per query, the way the one-shot CLI does.
    pub persistent: bool,
    /// Persistent mode only: percentage of queries after which the worker drops its TCP
    /// connection without a goodbye (an agent killed mid-query) and reconnects. The run then
    /// checks that the gateway stays healthy and retires every session.
    pub churn_percent: u8,
    /// How many worktrees to fork: a multiple of four, every [`WorktreeKind`] that many times
    /// over four (#406).
    pub worktrees: usize,
}

impl Default for DivergentBenchConfig {
    fn default() -> Self {
        Self {
            remote: SocketAddr::from(([127, 0, 0, 1], 9400)),
            base_repo: None,
            workdir: None,
            workers: 12,
            queries_per_worker: 5,
            keep_workdir: false,
            mode: WorkspaceMode::Isolated,
            persistent: false,
            churn_percent: 0,
            worktrees: 4,
        }
    }
}

/// Outcome of a single LSP query issued by a worker against one worktree.
#[derive(Debug, Clone)]
pub struct QueryOutcome {
    pub worker_id: usize,
    pub kind: WorktreeKind,
    pub latency: Duration,
    pub ok: bool,
    /// On success: the hover text returned by the gateway. On failure: the error message.
    pub detail: String,
}

/// What each worktree's hover text must (not) contain.
#[derive(Debug, Clone)]
pub struct Expectations {
    pub symbol: String,
    pub marker: String,
    pub untracked_symbol: String,
}

impl Expectations {
    pub fn for_target(target: &DivergentTarget) -> Self {
        Self {
            symbol: target.symbol.clone(),
            marker: target.language.marker().to_string(),
            untracked_symbol: target.language.untracked_symbol().to_string(),
        }
    }
}

/// Correctness verdict for one worktree kind, aggregated across all its query outcomes.
#[derive(Debug, Clone)]
pub struct VerificationResult {
    pub kind: WorktreeKind,
    pub passed: bool,
    pub message: String,
    /// Number of successful responses that violated the invariant.
    pub violations: usize,
    /// A violating (or, when none, representative) hover excerpt for diagnosis.
    pub sample: String,
}

/// Volume of the initial full workspace sync sent to the gateway.
#[derive(Debug, Clone)]
pub struct SyncSummary {
    pub workspace_name: String,
    pub files: usize,
    pub bytes: u64,
    pub duration: Duration,
}
