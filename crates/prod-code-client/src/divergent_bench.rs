//! Multi-worktree divergence and correctness benchmark.
//!
//! Simulates a fleet of AI coding agents operating on the *same* base repository through
//! several `git worktree`s that have diverged from one another:
//!
//! - **Master**: untouched base checkout.
//! - **Worktree A**: a function signature change on a real symbol (agent mid-refactor).
//! - **Worktree B**: a dependency manifest change in `Cargo.toml` / `go.mod` (agent bumping deps).
//! - **Worktree C**: a brand-new untracked file introducing a symbol (agent scratch file).
//!
//! The target symbol is discovered from the base repository itself (first single-line
//! `pub fn` / `func` in a tracked, non-test source file), so the benchmark exercises the real
//! crate/package graph the gateway indexes rather than a synthetic side crate that no engine
//! would load. When no base repository is given, a disposable scratch crate is created with a
//! root `Cargo.toml` so the same discovery path applies.
//!
//! Before any query, each server workspace receives one full sync (tracked source + manifests,
//! exactly what `prod-code sync` sends on first contact). Every query then performs the
//! transparent dirty/untracked pre-flight sync a live editor session does, so worktree A's
//! modified file and worktree C's untracked file are visible to the remote engine.
//!
//! Two workspace modes are supported:
//!
//! - [`WorkspaceMode::Shared`]: all four worktrees hand the gateway the same base workspace
//!   name, so they coalesce onto one shared server workspace / one in-memory Salsa DB. This is
//!   what production worktrees do (`detect_workspace_name` resolves a worktree to its origin
//!   repository), and is the mode that can surface cross-worktree bleed.
//! - [`WorkspaceMode::Isolated`]: each worktree gets its own server workspace and engine.
//!
//! The benchmark fires queries from 10+ concurrent simulated workers, round-robining across
//! the four worktrees, and asserts that every hover response reflects the querying worktree's
//! own state. Latency percentiles, throughput, sync volume and verification pass/fail are
//! reported at the end.

use crate::LatencyStats;
use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_util::codec::Framed;
use url::Url;

/// Minimum number of concurrent simulated workers required by the benchmark contract.
pub const MIN_WORKERS: usize = 10;

/// Suffix appended to the base repository name to form the origin clone / server workspace
/// name, keeping benchmark traffic away from the real shared workspace of the same repository.
pub const BENCH_WORKSPACE_SUFFIX: &str = "-divergent-bench";

const FIXTURE_REPO_NAME: &str = "fixture";
const FIXTURE_CARGO_TOML: &str = "[package]\nname = \"divergent-bench-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n";
const FIXTURE_LIB_RS: &str = "pub fn compute_signal(input: i64) -> i64 {\n    input * 2\n}\n";

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

    fn manifest(&self) -> &'static str {
        match self {
            Language::Rust => "Cargo.toml",
            Language::Go => "go.mod",
        }
    }

    fn extension(&self) -> &'static str {
        match self {
            Language::Rust => "rs",
            Language::Go => "go",
        }
    }

    /// Extra parameter appended to the target signature in worktree A.
    fn marker_param(&self) -> &'static str {
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

    fn untracked_file_name(&self) -> &'static str {
        match self {
            Language::Rust => "divergent_untracked.rs",
            Language::Go => "divergent_untracked.go",
        }
    }

    fn manifest_touch_line(&self) -> &'static str {
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

    fn dir_name(&self) -> &'static str {
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
    /// Base git repository to fork worktrees from (e.g. a BTCR or CodeHaus checkout).
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

/// Full benchmark report: latency percentiles, throughput, and correctness verification.
#[derive(Debug, Clone)]
pub struct DivergentBenchReport {
    pub language: Language,
    pub mode: WorkspaceMode,
    pub persistent: bool,
    pub workspace_name: String,
    pub target: DivergentTarget,
    pub initial_syncs: Vec<SyncSummary>,
    pub total_queries: usize,
    pub total_errors: usize,
    pub elapsed: Duration,
    pub qps: f64,
    pub latency: LatencyStats,
    pub latency_by_kind: BTreeMap<WorktreeKind, LatencyStats>,
    /// Failed query count and first error message per worktree kind.
    pub errors_by_kind: BTreeMap<WorktreeKind, (usize, String)>,
    pub verifications: Vec<VerificationResult>,
    pub all_passed: bool,
}

impl DivergentBenchReport {
    pub fn print(&self) {
        println!("\n📊 Divergent Worktree Benchmark Results:");
        println!("────────────────────────────────────────────────────────────────");
        println!("Language:            {}", self.language.label());
        println!("Workspace Mode:      {}", self.mode.label());
        println!(
            "Session Model:       {}",
            if self.persistent {
                "persistent (one session per worker)"
            } else {
                "connect per query"
            }
        );
        println!("Server Workspace:    {}", self.workspace_name);
        println!(
            "Target Symbol:       {} ({}:{})",
            self.target.symbol,
            self.target.file_rel.display(),
            self.target.line + 1
        );
        for sync in &self.initial_syncs {
            println!(
                "Initial Sync:        {} files, {:.1} KB in {:.0} ms -> {}",
                sync.files,
                sync.bytes as f64 / 1024.0,
                sync.duration.as_secs_f64() * 1000.0,
                sync.workspace_name
            );
        }
        println!("────────────────────────────────────────────────────────────────");
        println!("Elapsed Time:        {:.2}s", self.elapsed.as_secs_f64());
        println!("Completed Queries:   {}", self.total_queries);
        println!("Errors:              {}", self.total_errors);
        println!("Throughput:          {:.1} QPS", self.qps);
        println!("Latency (min):       {:.2} ms", self.latency.min_ms);
        println!("Latency (p50):       {:.2} ms", self.latency.p50_ms);
        println!("Latency (p95):       {:.2} ms", self.latency.p95_ms);
        println!("Latency (p99):       {:.2} ms", self.latency.p99_ms);
        println!("Latency (max):       {:.2} ms", self.latency.max_ms);
        for (kind, stats) in &self.latency_by_kind {
            println!(
                "  {:<22} n={:<4} p50={:.2} ms  p95={:.2} ms  p99={:.2} ms",
                kind.label(),
                stats.count,
                stats.p50_ms,
                stats.p95_ms,
                stats.p99_ms
            );
        }
        for (kind, (count, first)) in &self.errors_by_kind {
            println!(
                "  {:<22} errors={:<3} first: {}",
                kind.label(),
                count,
                excerpt(first)
            );
        }
        println!("────────────────────────────────────────────────────────────────");
        println!("Correctness Verification (zero cross-worktree bleed):");
        for v in &self.verifications {
            let mark = if v.passed { "✅ PASS" } else { "❌ FAIL" };
            println!("  {mark}  [{}] {}", v.kind.label(), v.message);
            if !v.passed {
                println!(
                    "          violations={} sample: {}",
                    v.violations,
                    excerpt(&v.sample)
                );
            }
        }
        println!("────────────────────────────────────────────────────────────────");
        println!(
            "Overall Status:      {}",
            if self.all_passed { "PASS" } else { "FAIL" }
        );
    }
}

fn excerpt(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 160 {
        let cut: String = flat.chars().take(160).collect();
        format!("{cut}…")
    } else {
        flat
    }
}

fn run_git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn `git {}` in {:?}", args.join(" "), dir))?;
    if !output.status.success() {
        bail!(
            "`git {}` in {:?} failed: {}",
            args.join(" "),
            dir,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn write_fixture_crate(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("Cargo.toml"), FIXTURE_CARGO_TOML)?;
    std::fs::write(root.join("src/lib.rs"), FIXTURE_LIB_RS)?;
    Ok(())
}

/// Name of the origin clone and of the shared server workspace for `base_repo`.
pub fn bench_workspace_name(base_repo: Option<&Path>) -> String {
    let repo_name = base_repo
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or(FIXTURE_REPO_NAME);
    format!("{repo_name}{BENCH_WORKSPACE_SUFFIX}")
}

/// Prepares the origin repository: clones `base_repo` locally (leaving the original untouched)
/// or initializes a disposable scratch repo with a root fixture crate. The clone directory is
/// named after the benchmark workspace so worktrees created from it resolve to that name.
fn prepare_origin(base_repo: Option<&Path>, workdir: &Path, name: &str) -> Result<PathBuf> {
    let origin = workdir.join(name);
    if origin.exists() {
        bail!("origin path {origin:?} already exists; use a fresh workdir");
    }
    match base_repo {
        Some(src) => {
            let src_str = src
                .to_str()
                .ok_or_else(|| anyhow!("base repo path must be valid UTF-8: {:?}", src))?;
            let origin_str = origin
                .to_str()
                .ok_or_else(|| anyhow!("workdir path must be valid UTF-8: {:?}", origin))?;
            run_git(
                workdir,
                &[
                    "clone",
                    "--quiet",
                    "--local",
                    "--no-hardlinks",
                    src_str,
                    origin_str,
                ],
            )?;
        }
        None => {
            std::fs::create_dir_all(&origin)?;
            run_git(&origin, &["init", "--quiet", "--initial-branch=main"])?;
            run_git(
                &origin,
                &["config", "user.email", "divergent-bench@prod.codes"],
            )?;
            run_git(
                &origin,
                &["config", "user.name", "prod-code divergent-bench"],
            )?;
            write_fixture_crate(&origin)?;
            run_git(&origin, &["add", "-A"])?;
            run_git(
                &origin,
                &[
                    "commit",
                    "--quiet",
                    "-m",
                    "divergent-bench: seed fixture crate",
                ],
            )?;
        }
    }
    Ok(origin)
}

/// Detects the base repository language from its root manifest.
pub fn detect_language(root: &Path) -> Result<Language> {
    if root.join(Language::Rust.manifest()).exists() {
        Ok(Language::Rust)
    } else if root.join(Language::Go.manifest()).exists() {
        Ok(Language::Go)
    } else {
        bail!(
            "no Cargo.toml or go.mod at {root:?}; the divergent benchmark needs a Rust or Go repository root"
        )
    }
}

fn is_candidate_source(rel: &str, language: Language) -> bool {
    let ext_ok = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e == language.extension());
    if !ext_ok {
        return false;
    }
    let lower = rel.to_ascii_lowercase();
    let excluded_dirs = [
        "test",
        "bench",
        "example",
        "vendor",
        "target",
        "node_modules",
        "third_party",
        "proto",
        "generated",
        ".bak",
    ];
    if lower
        .split('/')
        .any(|seg| excluded_dirs.iter().any(|ex| seg.contains(ex)))
    {
        return false;
    }
    match language {
        Language::Rust => !lower.ends_with("build.rs"),
        Language::Go => !lower.ends_with("_test.go") && !lower.ends_with(".pb.go"),
    }
}

/// Finds a single-line function signature on `line` and returns `(symbol, name_col)`.
fn parse_signature_line(line: &str, language: Language) -> Option<(String, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let after_keyword = match language {
        Language::Rust => trimmed.strip_prefix("pub fn ")?,
        Language::Go => {
            let rest = trimmed.strip_prefix("func ")?;
            if rest.starts_with('(') {
                return None; // method receiver; keep to plain functions
            }
            rest
        }
    };
    let name_end = after_keyword.find(['(', '<'])?;
    let name = &after_keyword[..name_end];
    // Entry points hover as package/crate documentation rather than as a signature.
    if matches!(name, "main" | "init") {
        return None;
    }
    if name.is_empty()
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || name.starts_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    if !trimmed.contains('(') || !trimmed.trim_end().ends_with('{') {
        return None;
    }
    let open = trimmed.find('(')?;
    let close = matching_paren(trimmed, open)?;
    if trimmed[open..=close].contains("...") {
        return None;
    }
    let name_col = indent + (trimmed.len() - after_keyword.len());
    let _ = close;
    Some((name.to_string(), name_col))
}

fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in text.char_indices().skip(open) {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

/// Discovers the first single-line top-level function in a tracked, non-test source file.
pub fn discover_target(root: &Path, language: Language) -> Result<DivergentTarget> {
    let listing = run_git(root, &["ls-files", "-z"])?;
    let mut candidates: Vec<&str> = listing
        .split('\0')
        .filter(|rel| !rel.is_empty() && is_candidate_source(rel, language))
        .collect();
    // Prefer conventional source roots so the symbol sits inside the indexed crate/package tree.
    candidates.sort_by_key(|rel| {
        let depth = rel.matches('/').count();
        let in_src = rel.starts_with("src/") || rel.contains("/src/");
        (if in_src { 0 } else { 1 }, depth, rel.to_string())
    });

    for rel in candidates {
        let Ok(content) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if language == Language::Rust && line.trim_start().starts_with("#[cfg(test)]") {
                break;
            }
            if let Some((symbol, _)) = parse_signature_line(line, language) {
                return Ok(DivergentTarget {
                    language,
                    file_rel: PathBuf::from(rel),
                    symbol,
                    line: idx,
                });
            }
        }
    }
    bail!(
        "no single-line `{}` signature found in tracked {} sources under {root:?}",
        match language {
            Language::Rust => "pub fn",
            Language::Go => "func",
        },
        language.label()
    )
}

/// Rewrites `line` so the parameter list ends with the language's marker parameter.
pub fn mutate_signature(line: &str, language: Language) -> Result<String> {
    let open = line
        .find('(')
        .ok_or_else(|| anyhow!("signature has no parameter list: {line}"))?;
    let close =
        matching_paren(line, open).ok_or_else(|| anyhow!("unbalanced parameter list: {line}"))?;
    let params = line[open + 1..close].trim();
    let marker = language.marker_param();
    let new_params = if params.is_empty() {
        marker.to_string()
    } else if params.ends_with(',') {
        format!("{params} {marker}")
    } else {
        format!("{params}, {marker}")
    };
    Ok(format!(
        "{}({}){}",
        &line[..open],
        new_params,
        &line[close + 1..]
    ))
}

fn go_package_name(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        line.trim()
            .strip_prefix("package ")
            .map(|p| p.split_whitespace().next().unwrap_or("").to_string())
    })
}

fn create_worktree(origin: &Path, workdir: &Path, kind: WorktreeKind) -> Result<PathBuf> {
    let path = workdir.join(kind.dir_name());
    let branch = format!("divergent-bench-{}", kind.dir_name());
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow!("worktree path must be valid UTF-8: {:?}", path))?;
    run_git(
        origin,
        &[
            "worktree", "add", "--quiet", "-b", &branch, path_str, "HEAD",
        ],
    )?;
    Ok(path)
}

/// Applies the controlled mutation for `kind` inside `root`, returning the file the benchmark
/// should query against this worktree and the symbol to hover.
fn apply_mutation(
    root: &Path,
    kind: WorktreeKind,
    target: &DivergentTarget,
) -> Result<(PathBuf, String)> {
    let target_file = root.join(&target.file_rel);
    let language = target.language;

    match kind {
        WorktreeKind::Master => Ok((target_file, target.symbol.clone())),
        WorktreeKind::SignatureChange => {
            let content = std::fs::read_to_string(&target_file)?;
            let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
            let line = lines
                .get_mut(target.line)
                .ok_or_else(|| anyhow!("target line {} out of range", target.line))?;
            *line = mutate_signature(line, language)?;
            let mut rewritten = lines.join("\n");
            if content.ends_with('\n') {
                rewritten.push('\n');
            }
            std::fs::write(&target_file, rewritten)?;
            Ok((target_file, target.symbol.clone()))
        }
        WorktreeKind::DependencyChange => {
            let manifest = root.join(language.manifest());
            let mut content = std::fs::read_to_string(&manifest)?;
            content.push_str(language.manifest_touch_line());
            std::fs::write(&manifest, content)?;
            Ok((target_file, target.symbol.clone()))
        }
        WorktreeKind::UntrackedFile => {
            let dir = target_file
                .parent()
                .ok_or_else(|| anyhow!("target file has no parent: {target_file:?}"))?;
            let untracked = dir.join(language.untracked_file_name());
            let symbol = language.untracked_symbol();
            let body = match language {
                Language::Rust => {
                    // A new Rust file is only analyzable once the crate declares it as a module,
                    // which is what an agent does right after creating a scratch file.
                    let mut owner = std::fs::read_to_string(&target_file)?;
                    if !owner.ends_with('\n') {
                        owner.push('\n');
                    }
                    owner.push_str(
                        "\n#[path = \"divergent_untracked.rs\"]\nmod divergent_untracked;\n",
                    );
                    std::fs::write(&target_file, owner)?;
                    format!(
                        "pub fn {symbol}() -> &'static str {{\n    \"divergent-bench-marker\"\n}}\n"
                    )
                }
                Language::Go => {
                    let package = go_package_name(&std::fs::read_to_string(&target_file)?)
                        .ok_or_else(|| anyhow!("no package clause in {target_file:?}"))?;
                    format!(
                        "package {package}\n\n// {symbol} is introduced by the divergent benchmark.\nfunc {symbol}() string {{\n\treturn \"divergent-bench-marker\"\n}}\n"
                    )
                }
            };
            std::fs::write(&untracked, body)?;
            Ok((untracked, symbol.to_string()))
        }
    }
}

/// Creates the origin repository, discovers the target symbol, and materializes the four
/// diverged, mutated worktrees.
pub fn setup(
    base_repo: Option<&Path>,
    workdir: &Path,
    mode: WorkspaceMode,
) -> Result<DivergenceSetup> {
    let workspace_name = bench_workspace_name(base_repo);
    let origin = prepare_origin(base_repo, workdir, &workspace_name)?;
    let language = detect_language(&origin)?;
    let target = discover_target(&origin, language)?;

    let mut worktrees = Vec::with_capacity(4);
    for kind in WorktreeKind::all() {
        let root = create_worktree(&origin, workdir, kind)?;
        let (query_file, symbol) = apply_mutation(&root, kind, &target)?;
        let wt_workspace_name = match mode {
            WorkspaceMode::Shared => workspace_name.clone(),
            WorkspaceMode::Isolated => format!("{workspace_name}-{}", kind.dir_name()),
        };
        worktrees.push(DivergentWorktree {
            kind,
            root,
            query_file,
            symbol,
            workspace_name: wt_workspace_name,
        });
    }

    Ok(DivergenceSetup {
        origin,
        workspace_name,
        target,
        worktrees,
    })
}

/// Position of `symbol`'s definition (`fn name(` / `func name(`), falling back to its first
/// occurrence. A doc comment that mentions the symbol must not win over the declaration.
fn locate_symbol(content: &str, symbol: &str) -> Result<(u32, u32)> {
    let mut fallback = None;
    for (idx, line) in content.lines().enumerate() {
        let mut search_from = 0;
        while let Some(rel) = line[search_from..].find(symbol) {
            let col = search_from + rel;
            let before = line[..col].trim_end();
            let after = &line[col + symbol.len()..];
            let is_definition = (before.ends_with("fn") || before.ends_with("func"))
                && after.starts_with(['(', '<']);
            if is_definition {
                return Ok((idx as u32, col as u32));
            }
            fallback.get_or_insert((idx as u32, col as u32));
            search_from = col + symbol.len();
        }
    }
    fallback.ok_or_else(|| anyhow!("symbol `{symbol}` not found in file content"))
}

fn extract_hover_text(result: &serde_json::Value) -> String {
    if let Some(contents) = result.get("contents") {
        if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
            return value.to_string();
        }
        if let Some(arr) = contents.as_array() {
            return arr
                .iter()
                .filter_map(|item| {
                    item.get("value")
                        .and_then(|v| v.as_str())
                        .or_else(|| item.as_str())
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
        if let Some(s) = contents.as_str() {
            return s.to_string();
        }
    }
    result.to_string()
}

async fn read_response_matching_id(
    framed: &mut Framed<TcpStream, ProdCodeCodec>,
    id: i64,
    query_timeout: Duration,
) -> Result<serde_json::Value> {
    let deadline = Instant::now() + query_timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("timed out waiting for response id={id}");
        }
        match timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(payload)))) => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&payload)
                    && val.get("id").and_then(|v| v.as_i64()) == Some(id)
                {
                    return Ok(val);
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => bail!("frame decode error: {e}"),
            Ok(None) => bail!("remote gateway closed connection prematurely"),
            Err(_) => bail!("timed out waiting for response id={id}"),
        }
    }
}

/// Sends one full workspace sync (tracked sources and manifests plus current dirty/untracked
/// files) for `root` to the gateway under `workspace_name`, the way `prod-code sync` does on
/// first contact. The persisted watermark is cleared afterwards so no state outlives the run.
pub async fn initial_sync(
    remote: SocketAddr,
    root: &Path,
    workspace_name: &str,
) -> Result<SyncSummary> {
    prod_code_mcp::sync::clear_sync_cache(root);
    let identity = prod_code_mcp::sync::WorkspaceIdentity {
        name: workspace_name.to_string(),
        base: None,
    };
    let start = Instant::now();
    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let outcome =
        prod_code_mcp::sync::push_workspace_sync(&mut framed, root, &identity, None).await?;
    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "divergent-bench initial sync finished".to_string(),
        })
        .await;
    Ok(SyncSummary {
        workspace_name: workspace_name.to_string(),
        files: outcome.files_updated,
        bytes: outcome.bytes_transferred as u64,
        duration: start.elapsed(),
    })
}

/// Opens a gateway session for `wt`: connect, pre-flight sync, handshake, initialize.
async fn open_session(
    remote: SocketAddr,
    wt: &DivergentWorktree,
    client_name: String,
) -> Result<Framed<TcpStream, ProdCodeCodec>> {
    let ws_root_str = wt.root.to_string_lossy().to_string();
    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    // Transparent pre-flight sync before the handshake, exactly like a live editor session: a
    // mutated-but-uncommitted file (worktree A), an untracked scratch file (worktree C) or
    // commits since the last sync reach the gateway before engine detection and the query.
    let identity = prod_code_mcp::sync::WorkspaceIdentity {
        name: wt.workspace_name.clone(),
        base: None,
    };
    prod_code_mcp::sync::push_workspace_sync(&mut framed, &wt.root, &identity, None).await?;

    framed
        .send(WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: PROTOCOL_VERSION,
            client_name,
            client_pid: std::process::id(),
            auth_token: None,
            client_workspace_root: ws_root_str.clone(),
            preferred_engine: None,
            base_workspace_name: Some(wt.workspace_name.clone()),
        }))
        .await?;

    match framed.next().await {
        Some(Ok(WireMessage::HandshakeResponse(_))) => {}
        other => bail!("unexpected handshake response: {other:?}"),
    }

    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": format!("file://{ws_root_str}"),
            "capabilities": {}
        }
    });
    framed
        .send(WireMessage::LspPayload(init_req.to_string()))
        .await?;
    read_response_matching_id(&mut framed, 1, Duration::from_secs(10)).await?;

    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    framed
        .send(WireMessage::LspPayload(initialized.to_string()))
        .await?;
    Ok(framed)
}

/// One didOpen/hover/didClose round on an open session; returns the hover text for `wt.symbol`.
async fn hover_in_session(
    framed: &mut Framed<TcpStream, ProdCodeCodec>,
    wt: &DivergentWorktree,
    language: Language,
    request_id: i64,
) -> Result<String> {
    let file_path = &wt.query_file;
    let content = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("failed to read {file_path:?}"))?;
    let (line, col) = locate_symbol(&content, &wt.symbol)?;
    let file_uri = Url::from_file_path(file_path)
        .map_err(|_| anyhow!("invalid file path for URI: {:?}", file_path))?
        .to_string();

    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": file_uri,
                "languageId": language.label(),
                "version": 1,
                "text": content
            }
        }
    });
    framed
        .send(WireMessage::LspPayload(did_open.to_string()))
        .await?;

    let hover_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "textDocument/hover",
        "params": {
            "textDocument": { "uri": file_uri },
            "position": { "line": line, "character": col }
        }
    });
    framed
        .send(WireMessage::LspPayload(hover_req.to_string()))
        .await?;
    let response = read_response_matching_id(framed, request_id, Duration::from_secs(30)).await?;
    let result = response.get("result").unwrap_or(&serde_json::Value::Null);
    if result.is_null() {
        bail!(
            "hover returned null for {} in {}",
            wt.symbol,
            file_path.display()
        );
    }
    let hover_text = extract_hover_text(result);

    let did_close = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didClose",
        "params": { "textDocument": { "uri": file_uri } }
    });
    let _ = framed
        .send(WireMessage::LspPayload(did_close.to_string()))
        .await;
    Ok(hover_text)
}

async fn close_session(mut framed: Framed<TcpStream, ProdCodeCodec>) {
    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "divergent-bench session finished".to_string(),
        })
        .await;
}

/// Connect-per-query round: open a session, hover once, close it.
async fn query_once(
    remote: SocketAddr,
    wt: &DivergentWorktree,
    language: Language,
    client_name: String,
) -> Result<String> {
    let mut framed = open_session(remote, wt, client_name).await?;
    let text = hover_in_session(&mut framed, wt, language, 2).await;
    close_session(framed).await;
    text
}

type HoverPredicate<'a> = Box<dyn Fn(&str) -> bool + 'a>;

/// Groups query outcomes by worktree kind and asserts each kind's expected correctness
/// invariant, catching any cross-worktree bleed.
pub fn verify(outcomes: &[QueryOutcome], expect: &Expectations) -> Vec<VerificationResult> {
    let symbol = expect.symbol.as_str();
    let marker = expect.marker.as_str();
    let untracked = expect.untracked_symbol.as_str();

    WorktreeKind::all()
        .into_iter()
        .map(|kind| {
            let relevant: Vec<&QueryOutcome> =
                outcomes.iter().filter(|o| o.kind == kind && o.ok).collect();

            if relevant.is_empty() {
                let sample = outcomes
                    .iter()
                    .find(|o| o.kind == kind)
                    .map(|o| o.detail.clone())
                    .unwrap_or_default();
                return VerificationResult {
                    kind,
                    passed: false,
                    message: format!("no successful responses for {}", kind.label()),
                    violations: 0,
                    sample,
                };
            }

            let (predicate, message): (HoverPredicate, &str) = match kind {
                WorktreeKind::Master => (
                    Box::new(|d: &str| d.contains(symbol) && !d.contains(marker)),
                    "master must show the unmutated base signature with no marker bleed from worktree A",
                ),
                WorktreeKind::SignatureChange => (
                    Box::new(|d: &str| d.contains(symbol) && d.contains(marker)),
                    "worktree A must show the mutated signature carrying the marker parameter",
                ),
                WorktreeKind::DependencyChange => (
                    Box::new(|d: &str| d.contains(symbol) && !d.contains(marker)),
                    "worktree B only touches the manifest; the signature must stay in base form",
                ),
                WorktreeKind::UntrackedFile => (
                    Box::new(|d: &str| d.contains(untracked)),
                    "worktree C must resolve the symbol from its untracked file",
                ),
            };

            let violating: Vec<&&QueryOutcome> =
                relevant.iter().filter(|o| !predicate(&o.detail)).collect();
            let sample = violating
                .first()
                .or(relevant.first().as_ref().map(|o| *o).as_ref())
                .map(|o| o.detail.clone())
                .unwrap_or_default();

            VerificationResult {
                kind,
                passed: violating.is_empty(),
                message: message.to_string(),
                violations: violating.len(),
                sample,
            }
        })
        .collect()
}

/// Runs the full divergent-worktree benchmark: sets up worktrees, syncs them to `config.remote`,
/// fires concurrent workers' queries, and verifies correctness. Requires at least
/// [`MIN_WORKERS`] concurrent workers to faithfully simulate a real agent fleet.
pub async fn run(config: DivergentBenchConfig) -> Result<DivergentBenchReport> {
    if config.workers < MIN_WORKERS {
        bail!(
            "divergent benchmark requires at least {MIN_WORKERS} concurrent workers, got {}",
            config.workers
        );
    }
    if config.queries_per_worker == 0 {
        bail!("queries_per_worker must be at least 1");
    }

    let mut owned_tempdir: Option<tempfile::TempDir> = None;
    let workdir = match &config.workdir {
        Some(p) => {
            std::fs::create_dir_all(p)
                .with_context(|| format!("failed to create workdir {p:?}"))?;
            p.clone()
        }
        None => {
            let dir = tempfile::tempdir().context("failed to create scratch workdir")?;
            let path = dir.path().to_path_buf();
            owned_tempdir = Some(dir);
            path
        }
    };

    let setup = setup(config.base_repo.as_deref(), &workdir, config.mode)?;
    let language = setup.target.language;
    let expect = Expectations::for_target(&setup.target);
    let worktrees = setup.worktrees;

    // One full sync per server workspace before the fleet starts querying.
    let mut initial_syncs = Vec::new();
    match config.mode {
        WorkspaceMode::Shared => {
            let master = worktrees
                .iter()
                .find(|w| w.kind == WorktreeKind::Master)
                .ok_or_else(|| anyhow!("master worktree missing"))?;
            initial_syncs
                .push(initial_sync(config.remote, &master.root, &master.workspace_name).await?);
        }
        WorkspaceMode::Isolated => {
            for wt in &worktrees {
                initial_syncs
                    .push(initial_sync(config.remote, &wt.root, &wt.workspace_name).await?);
            }
        }
    }

    let start = Instant::now();
    let mut handles = Vec::with_capacity(config.workers);
    for worker_id in 0..config.workers {
        let wt = worktrees[worker_id % worktrees.len()].clone();
        let remote = config.remote;
        let queries = config.queries_per_worker;
        let persistent = config.persistent;
        handles.push(tokio::spawn(async move {
            let mut outcomes = Vec::with_capacity(queries);
            let record = |outcomes: &mut Vec<QueryOutcome>, t0: Instant, res: Result<String>| {
                outcomes.push(match res {
                    Ok(detail) => QueryOutcome {
                        worker_id,
                        kind: wt.kind,
                        latency: t0.elapsed(),
                        ok: true,
                        detail,
                    },
                    Err(e) => QueryOutcome {
                        worker_id,
                        kind: wt.kind,
                        latency: t0.elapsed(),
                        ok: false,
                        detail: e.to_string(),
                    },
                });
            };
            if persistent {
                let client_name = format!("divergent-bench-worker-{worker_id}");
                match open_session(remote, &wt, client_name).await {
                    Ok(mut framed) => {
                        for q in 0..queries {
                            let t0 = Instant::now();
                            let res =
                                hover_in_session(&mut framed, &wt, language, 2 + q as i64).await;
                            record(&mut outcomes, t0, res);
                        }
                        close_session(framed).await;
                    }
                    Err(e) => {
                        for _ in 0..queries {
                            record(&mut outcomes, Instant::now(), Err(anyhow!("{e:#}")));
                        }
                    }
                }
            } else {
                for q in 0..queries {
                    let client_name = format!("divergent-bench-worker-{worker_id}-{q}");
                    let t0 = Instant::now();
                    let res = query_once(remote, &wt, language, client_name).await;
                    record(&mut outcomes, t0, res);
                }
            }
            outcomes
        }));
    }

    let mut all_outcomes = Vec::new();
    for handle in handles {
        let outcomes = handle
            .await
            .context("divergent-bench worker task panicked")?;
        all_outcomes.extend(outcomes);
    }
    let elapsed = start.elapsed();

    // The scratch worktrees are disposable; drop their persisted sync watermarks.
    for wt in &worktrees {
        prod_code_mcp::sync::clear_sync_cache(&wt.root);
    }

    if config.keep_workdir {
        // Prevent the scratch TempDir from deleting itself on drop so it can be inspected.
        if let Some(dir) = owned_tempdir.take() {
            let _ = dir.keep();
        }
    }

    let verifications = verify(&all_outcomes, &expect);
    let total_errors = all_outcomes.iter().filter(|o| !o.ok).count();
    let all_verified = verifications.iter().all(|v| v.passed);
    let all_passed = all_verified && total_errors == 0;

    let mut latencies_us: Vec<u64> = all_outcomes
        .iter()
        .filter(|o| o.ok)
        .map(|o| o.latency.as_micros() as u64)
        .collect();
    let latency = LatencyStats::from_micros(&mut latencies_us);

    let mut latency_by_kind = BTreeMap::new();
    for kind in WorktreeKind::all() {
        let mut samples: Vec<u64> = all_outcomes
            .iter()
            .filter(|o| o.ok && o.kind == kind)
            .map(|o| o.latency.as_micros() as u64)
            .collect();
        latency_by_kind.insert(kind, LatencyStats::from_micros(&mut samples));
    }

    let mut errors_by_kind = BTreeMap::new();
    for outcome in all_outcomes.iter().filter(|o| !o.ok) {
        let entry = errors_by_kind
            .entry(outcome.kind)
            .or_insert_with(|| (0usize, outcome.detail.clone()));
        entry.0 += 1;
    }

    let qps = if elapsed.as_secs_f64() > 0.0 {
        all_outcomes.len() as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    Ok(DivergentBenchReport {
        language,
        mode: config.mode,
        persistent: config.persistent,
        workspace_name: setup.workspace_name,
        target: setup.target,
        initial_syncs,
        total_queries: all_outcomes.len(),
        total_errors,
        elapsed,
        qps,
        latency,
        latency_by_kind,
        errors_by_kind,
        verifications,
        all_passed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_SIGNATURE: &str = "pub fn compute_signal(input: i64) -> i64";
    const MUTATED_SIGNATURE: &str =
        "pub fn compute_signal(input: i64, divergent_marker: i64) -> i64";

    fn expectations() -> Expectations {
        Expectations {
            symbol: "compute_signal".to_string(),
            marker: Language::Rust.marker().to_string(),
            untracked_symbol: Language::Rust.untracked_symbol().to_string(),
        }
    }

    fn synthetic(kind: WorktreeKind, ok: bool, detail: &str) -> QueryOutcome {
        QueryOutcome {
            worker_id: 0,
            kind,
            latency: Duration::from_millis(1),
            ok,
            detail: detail.to_string(),
        }
    }

    fn clean_outcomes() -> Vec<QueryOutcome> {
        vec![
            synthetic(WorktreeKind::Master, true, BASE_SIGNATURE),
            synthetic(WorktreeKind::SignatureChange, true, MUTATED_SIGNATURE),
            synthetic(WorktreeKind::DependencyChange, true, BASE_SIGNATURE),
            synthetic(
                WorktreeKind::UntrackedFile,
                true,
                "pub fn divergent_untracked_symbol() -> &'static str",
            ),
        ]
    }

    #[test]
    fn verify_passes_with_clean_divergence() {
        let results = verify(&clean_outcomes(), &expectations());
        assert!(results.iter().all(|r| r.passed), "{results:?}");
    }

    #[test]
    fn verify_catches_cross_worktree_bleed_into_master() {
        // Master's response contains worktree A's marker: a routing/bleed bug.
        let mut outcomes = clean_outcomes();
        outcomes[0] = synthetic(WorktreeKind::Master, true, MUTATED_SIGNATURE);
        let results = verify(&outcomes, &expectations());
        let master = results
            .iter()
            .find(|r| r.kind == WorktreeKind::Master)
            .unwrap();
        assert!(!master.passed);
        assert_eq!(master.violations, 1);
        assert!(master.sample.contains("divergent_marker"));
    }

    #[test]
    fn verify_catches_bleed_from_master_into_worktree_a() {
        let mut outcomes = clean_outcomes();
        outcomes[1] = synthetic(WorktreeKind::SignatureChange, true, BASE_SIGNATURE);
        let results = verify(&outcomes, &expectations());
        let a = results
            .iter()
            .find(|r| r.kind == WorktreeKind::SignatureChange)
            .unwrap();
        assert!(!a.passed);
    }

    #[test]
    fn verify_catches_untracked_symbol_not_resolved() {
        let mut outcomes = clean_outcomes();
        outcomes[3] = synthetic(WorktreeKind::UntrackedFile, true, "no symbol here");
        let results = verify(&outcomes, &expectations());
        let untracked = results
            .iter()
            .find(|r| r.kind == WorktreeKind::UntrackedFile)
            .unwrap();
        assert!(!untracked.passed);
    }

    #[test]
    fn verify_fails_when_no_successful_responses() {
        let outcomes = vec![synthetic(WorktreeKind::Master, false, "connection refused")];
        let results = verify(&outcomes, &expectations());
        for kind in WorktreeKind::all() {
            let r = results.iter().find(|r| r.kind == kind).unwrap();
            assert!(!r.passed);
        }
        assert!(results[0].sample.contains("connection refused"));
    }

    #[test]
    fn locate_symbol_finds_line_and_column() {
        let content = "line one\npub fn compute_signal(input: i64) -> i64 {\n";
        let (line, col) = locate_symbol(content, "compute_signal").unwrap();
        assert_eq!(line, 1);
        assert_eq!(
            col,
            content
                .lines()
                .nth(1)
                .unwrap()
                .find("compute_signal")
                .unwrap() as u32
        );
    }

    #[test]
    fn locate_symbol_prefers_definition_over_doc_comment() {
        let content = "package main\n\n// DivergentUntrackedSymbol is introduced by the benchmark.\nfunc DivergentUntrackedSymbol() string {\n";
        let (line, col) = locate_symbol(content, "DivergentUntrackedSymbol").unwrap();
        assert_eq!(line, 3);
        assert_eq!(col, 5);
        let rust = "/// compute_signal docs\npub fn compute_signal(input: i64) -> i64 {\n";
        assert_eq!(locate_symbol(rust, "compute_signal").unwrap(), (1, 7));
    }

    #[test]
    fn locate_symbol_missing_errors() {
        assert!(locate_symbol("nothing to see here", "compute_signal").is_err());
    }

    #[test]
    fn extract_hover_text_from_plain_value() {
        let result = serde_json::json!({ "contents": { "value": "hello world" } });
        assert_eq!(extract_hover_text(&result), "hello world");
    }

    #[test]
    fn extract_hover_text_from_array() {
        let result = serde_json::json!({ "contents": [ { "value": "a" }, "b" ] });
        assert_eq!(extract_hover_text(&result), "a\nb");
    }

    #[test]
    fn parse_signature_line_rust_and_go() {
        assert_eq!(
            parse_signature_line("pub fn compute_signal(input: i64) -> i64 {", Language::Rust),
            Some(("compute_signal".to_string(), 7))
        );
        assert_eq!(
            parse_signature_line("    pub fn new<T: Clone>(x: T) -> Self {", Language::Rust),
            Some(("new".to_string(), 11))
        );
        assert_eq!(
            parse_signature_line("pub fn trait_item(x: i64) -> i64;", Language::Rust),
            None
        );
        assert_eq!(
            parse_signature_line("pub fn multi_line(", Language::Rust),
            None
        );
        assert_eq!(
            parse_signature_line("func Compute(a int, b string) error {", Language::Go),
            Some(("Compute".to_string(), 5))
        );
        assert_eq!(
            parse_signature_line("func (s *Server) Method() error {", Language::Go),
            None
        );
        assert_eq!(parse_signature_line("func main() {", Language::Go), None);
        assert_eq!(parse_signature_line("func init() {", Language::Go), None);
        assert_eq!(
            parse_signature_line("pub fn main() {", Language::Rust),
            None
        );
    }

    #[test]
    fn mutate_signature_appends_marker_parameter() {
        assert_eq!(
            mutate_signature("pub fn compute_signal(input: i64) -> i64 {", Language::Rust).unwrap(),
            MUTATED_SIGNATURE.to_string() + " {"
        );
        assert_eq!(
            mutate_signature("pub fn empty() {", Language::Rust).unwrap(),
            "pub fn empty(divergent_marker: i64) {"
        );
        assert_eq!(
            mutate_signature("pub fn nested(f: fn(i64) -> i64) -> i64 {", Language::Rust).unwrap(),
            "pub fn nested(f: fn(i64) -> i64, divergent_marker: i64) -> i64 {"
        );
        assert_eq!(
            mutate_signature("func Compute(a int) error {", Language::Go).unwrap(),
            "func Compute(a int, divergentMarker int) error {"
        );
    }

    #[test]
    fn candidate_source_filter_skips_tests_and_generated() {
        assert!(is_candidate_source("src/lib.rs", Language::Rust));
        assert!(!is_candidate_source("tests/it.rs", Language::Rust));
        assert!(!is_candidate_source("benches/b.rs", Language::Rust));
        assert!(!is_candidate_source("build.rs", Language::Rust));
        assert!(!is_candidate_source("src/lib.go", Language::Rust));
        assert!(is_candidate_source("cmd/server/main.go", Language::Go));
        assert!(!is_candidate_source(
            "cmd/server/main_test.go",
            Language::Go
        ));
        assert!(!is_candidate_source("api/v1/types.pb.go", Language::Go));
        assert!(!is_candidate_source("vendor/x/y.go", Language::Go));
    }

    #[test]
    fn setup_creates_four_worktrees_with_expected_mutations() {
        let tmp = tempfile::tempdir().unwrap();
        let setup = setup(None, tmp.path(), WorkspaceMode::Shared).expect("setup should succeed");
        assert_eq!(setup.worktrees.len(), 4);
        assert_eq!(setup.workspace_name, "fixture-divergent-bench");
        assert_eq!(setup.target.symbol, "compute_signal");
        assert_eq!(setup.target.file_rel, PathBuf::from("src/lib.rs"));
        assert!(setup.origin.join("Cargo.toml").exists());

        for wt in &setup.worktrees {
            assert!(wt.root.exists(), "{:?} should exist", wt.root);
            assert_eq!(wt.workspace_name, "fixture-divergent-bench");
            let content = std::fs::read_to_string(&wt.query_file).unwrap();
            match wt.kind {
                WorktreeKind::Master => assert!(content.contains(BASE_SIGNATURE)),
                WorktreeKind::SignatureChange => assert!(content.contains(MUTATED_SIGNATURE)),
                WorktreeKind::DependencyChange => assert!(content.contains(BASE_SIGNATURE)),
                WorktreeKind::UntrackedFile => {
                    assert!(content.contains("divergent_untracked_symbol"));
                    assert!(wt.query_file.ends_with("src/divergent_untracked.rs"));
                    let owner = std::fs::read_to_string(wt.root.join("src/lib.rs")).unwrap();
                    assert!(owner.contains("mod divergent_untracked;"));
                    assert!(owner.contains(BASE_SIGNATURE));
                }
            }
        }

        let manifest_of = |kind: WorktreeKind| {
            let root = &setup
                .worktrees
                .iter()
                .find(|w| w.kind == kind)
                .unwrap()
                .root;
            std::fs::read_to_string(root.join("Cargo.toml")).unwrap()
        };
        assert!(manifest_of(WorktreeKind::DependencyChange).contains("manifest touched"));
        assert!(!manifest_of(WorktreeKind::Master).contains("manifest touched"));
    }

    #[test]
    fn setup_isolated_mode_names_each_worktree_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let setup = setup(None, tmp.path(), WorkspaceMode::Isolated).unwrap();
        let names: Vec<&str> = setup
            .worktrees
            .iter()
            .map(|w| w.workspace_name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "fixture-divergent-bench-wt-master",
                "fixture-divergent-bench-wt-signature",
                "fixture-divergent-bench-wt-dependency",
                "fixture-divergent-bench-wt-untracked",
            ]
        );
    }

    #[tokio::test]
    async fn run_rejects_too_few_workers() {
        let config = DivergentBenchConfig {
            workers: MIN_WORKERS - 1,
            ..DivergentBenchConfig::default()
        };
        let err = run(config).await.unwrap_err();
        assert!(err.to_string().contains("at least"));
    }
}
