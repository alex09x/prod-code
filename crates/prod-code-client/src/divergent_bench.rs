//! Multi-worktree divergence and correctness benchmark.
//!
//! Simulates a fleet of AI coding agents operating on the *same* base repository through
//! several `git worktree`s that have diverged from one another:
//!
//! - **Master**: untouched base checkout.
//! - **Worktree A**: a function signature change (agent mid-refactor).
//! - **Worktree B**: a dependency manifest change in `Cargo.toml` (agent bumping a crate).
//! - **Worktree C**: a brand-new untracked file introducing a symbol (agent scratch file).
//!
//! It then fires LSP queries at the remote gateway from 10+ concurrent simulated workers,
//! round-robining across the four worktrees, and asserts that every response reflects the
//! querying worktree's own state with zero cross-worktree bleed (each worktree gets its own
//! `client_workspace_root`, so a leak means the gateway routed a response from the wrong
//! session/workspace). Latency percentiles, throughput, and verification pass/fail are
//! reported at the end.

use crate::LatencyStats;
use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, SyncRequest, WireMessage,
};
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

const FIXTURE_CRATE_DIR: &str = "fixture_crate";
const FIXTURE_FILE_REL: &str = "src/lib.rs";
const UNTRACKED_FILE_REL: &str = "src/untracked_marker.rs";
const FIXTURE_SYMBOL: &str = "compute_signal";
const UNTRACKED_SYMBOL: &str = "untracked_marker_symbol";

const BASE_SIGNATURE: &str = "pub fn compute_signal(input: i64) -> i64";
const MUTATED_SIGNATURE: &str = "pub fn compute_signal(input: i64, scale: i64) -> i64";

const FIXTURE_CARGO_TOML: &str = "[package]\nname = \"divergent-bench-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n";
const FIXTURE_LIB_RS: &str = "pub fn compute_signal(input: i64) -> i64 {\n    input * 2\n}\n";

/// Which of the four diverged workspaces a worktree/query/result belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// One isolated `git worktree`, mutated according to its [`WorktreeKind`], along with the
/// specific file/symbol the benchmark will query against it.
#[derive(Debug, Clone)]
pub struct DivergentWorktree {
    pub kind: WorktreeKind,
    pub root: PathBuf,
    pub query_file: PathBuf,
    pub symbol: &'static str,
}

/// The prepared origin repository plus its four diverged worktrees.
pub struct DivergenceSetup {
    pub origin: PathBuf,
    pub worktrees: Vec<DivergentWorktree>,
}

/// Configuration for a single divergent-benchmark run.
#[derive(Debug, Clone)]
pub struct DivergentBenchConfig {
    /// Address of the prod-code remote gateway to hammer with LSP queries.
    pub remote: SocketAddr,
    /// Base git repository to fork worktrees from (e.g. a BTCR or govcon-intel checkout).
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

/// Correctness verdict for one worktree kind, aggregated across all its query outcomes.
#[derive(Debug, Clone)]
pub struct VerificationResult {
    pub kind: WorktreeKind,
    pub passed: bool,
    pub message: String,
}

/// Full benchmark report: latency percentiles, throughput, and correctness verification.
#[derive(Debug, Clone)]
pub struct DivergentBenchReport {
    pub total_queries: usize,
    pub total_errors: usize,
    pub elapsed: Duration,
    pub qps: f64,
    pub latency: LatencyStats,
    pub verifications: Vec<VerificationResult>,
    pub all_passed: bool,
}

impl DivergentBenchReport {
    pub fn print(&self) {
        println!("\n📊 Divergent Worktree Benchmark Results:");
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
        println!("────────────────────────────────────────────────────────────────");
        println!("Correctness Verification (zero cross-worktree bleed):");
        for v in &self.verifications {
            let mark = if v.passed { "✅ PASS" } else { "❌ FAIL" };
            println!("  {mark}  [{}] {}", v.kind.label(), v.message);
        }
        println!("────────────────────────────────────────────────────────────────");
        println!(
            "Overall Status:      {}",
            if self.all_passed { "PASS" } else { "FAIL" }
        );
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
    let crate_dir = root.join(FIXTURE_CRATE_DIR);
    std::fs::create_dir_all(crate_dir.join("src"))?;
    std::fs::write(crate_dir.join("Cargo.toml"), FIXTURE_CARGO_TOML)?;
    std::fs::write(crate_dir.join(FIXTURE_FILE_REL), FIXTURE_LIB_RS)?;
    Ok(())
}

/// Prepares the origin repository: clones `base_repo` locally (leaving the original untouched)
/// or initializes a disposable scratch repo, then commits a deterministic fixture crate so the
/// benchmark has a known symbol/signature to mutate and verify regardless of what the base repo
/// actually contains.
fn prepare_origin(base_repo: Option<&Path>, workdir: &Path) -> Result<PathBuf> {
    let origin = workdir.join("origin");
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
        }
    }

    // Ensure a commit identity exists even in minimal/CI environments without global git config.
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
    Ok(origin)
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
/// should query against this worktree.
fn apply_mutation(root: &Path, kind: WorktreeKind) -> Result<PathBuf> {
    let crate_dir = root.join(FIXTURE_CRATE_DIR);
    let lib_rs = crate_dir.join(FIXTURE_FILE_REL);

    match kind {
        WorktreeKind::Master => Ok(lib_rs),
        WorktreeKind::SignatureChange => {
            std::fs::write(
                &lib_rs,
                format!("{MUTATED_SIGNATURE} {{\n    input * scale\n}}\n"),
            )?;
            Ok(lib_rs)
        }
        WorktreeKind::DependencyChange => {
            let cargo_toml = crate_dir.join("Cargo.toml");
            let mut content = std::fs::read_to_string(&cargo_toml)?;
            content.push_str("serde = { version = \"1.0\", features = [\"derive\"] }\n");
            std::fs::write(&cargo_toml, content)?;
            Ok(lib_rs)
        }
        WorktreeKind::UntrackedFile => {
            let untracked = crate_dir.join(UNTRACKED_FILE_REL);
            std::fs::write(
                &untracked,
                format!(
                    "pub fn {UNTRACKED_SYMBOL}() -> &'static str {{\n    \"divergent-bench-marker\"\n}}\n"
                ),
            )?;
            Ok(untracked)
        }
    }
}

fn symbol_for(kind: WorktreeKind) -> &'static str {
    match kind {
        WorktreeKind::UntrackedFile => UNTRACKED_SYMBOL,
        _ => FIXTURE_SYMBOL,
    }
}

/// Creates the origin repository and its four diverged, mutated worktrees.
pub fn setup(base_repo: Option<&Path>, workdir: &Path) -> Result<DivergenceSetup> {
    let origin = prepare_origin(base_repo, workdir)?;

    let mut worktrees = Vec::with_capacity(4);
    for kind in WorktreeKind::all() {
        let root = create_worktree(&origin, workdir, kind)?;
        let query_file = apply_mutation(&root, kind)?;
        worktrees.push(DivergentWorktree {
            kind,
            root,
            query_file,
            symbol: symbol_for(kind),
        });
    }

    Ok(DivergenceSetup { origin, worktrees })
}

fn locate_symbol(content: &str, symbol: &str) -> Result<(u32, u32)> {
    for (idx, line) in content.lines().enumerate() {
        if let Some(col) = line.find(symbol) {
            return Ok((idx as u32, col as u32));
        }
    }
    bail!("symbol `{symbol}` not found in file content")
}

fn extract_hover_text(result: &serde_json::Value) -> String {
    if let Some(contents) = result.get("contents") {
        if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
            return value.to_string();
        }
        if let Some(arr) = contents.as_array() {
            return arr
                .iter()
                .filter_map(|item| item.get("value").and_then(|v| v.as_str()))
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

/// Connects to `remote`, performs the full handshake/sync/initialize/hover/close cycle against
/// `file_path` within `ws_root`, and returns the hover text the gateway reports for `symbol`.
async fn query_once(
    remote: SocketAddr,
    ws_root: &Path,
    file_path: &Path,
    symbol: &str,
    client_name: String,
) -> Result<String> {
    let content = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("failed to read {file_path:?}"))?;
    let (line, col) = locate_symbol(&content, symbol)?;

    let file_uri = Url::from_file_path(file_path)
        .map_err(|_| anyhow!("invalid file path for URI: {:?}", file_path))?
        .to_string();
    let ws_root_str = ws_root.to_string_lossy().to_string();

    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    framed
        .send(WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: PROTOCOL_VERSION,
            client_name,
            client_pid: std::process::id(),
            auth_token: None,
            client_workspace_root: ws_root_str.clone(),
            preferred_engine: None,
            base_workspace_name: ws_root
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string()),
        }))
        .await?;

    match framed.next().await {
        Some(Ok(WireMessage::HandshakeResponse(_))) => {}
        other => bail!("unexpected handshake response: {other:?}"),
    }

    // Transparent pre-flight sync of dirty/untracked files, exactly like a live editor session,
    // so a mutated-but-uncommitted file (worktree A) or an untracked scratch file (worktree C)
    // is visible to the remote engine before we query it.
    if let Ok(dirty) = prod_code_mcp::sync::collect_dirty_files(ws_root)
        && !dirty.is_empty()
    {
        framed
            .send(WireMessage::SyncRequest(SyncRequest {
                client_workspace_root: ws_root_str.clone(),
                files: dirty,
                clean_others: false,
                base_workspace_name: None,
            }))
            .await?;
        let _ = framed.next().await;
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

    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": file_uri,
                "languageId": "rust",
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
        "id": 2,
        "method": "textDocument/hover",
        "params": {
            "textDocument": { "uri": file_uri },
            "position": { "line": line, "character": col }
        }
    });
    framed
        .send(WireMessage::LspPayload(hover_req.to_string()))
        .await?;
    let response = read_response_matching_id(&mut framed, 2, Duration::from_secs(30)).await?;
    let hover_text = extract_hover_text(response.get("result").unwrap_or(&serde_json::Value::Null));

    let did_close = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didClose",
        "params": { "textDocument": { "uri": file_uri } }
    });
    let _ = framed
        .send(WireMessage::LspPayload(did_close.to_string()))
        .await;
    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "divergent-bench query finished".to_string(),
        })
        .await;

    Ok(hover_text)
}

fn check_all(outcomes: &[&QueryOutcome], pred: impl Fn(&str) -> bool) -> bool {
    outcomes.iter().all(|o| pred(&o.detail))
}

/// Groups query outcomes by worktree kind and asserts each kind's expected correctness
/// invariant, catching any cross-worktree bleed.
pub fn verify(outcomes: &[QueryOutcome]) -> Vec<VerificationResult> {
    WorktreeKind::all()
        .into_iter()
        .map(|kind| {
            let relevant: Vec<&QueryOutcome> =
                outcomes.iter().filter(|o| o.kind == kind && o.ok).collect();

            if relevant.is_empty() {
                return VerificationResult {
                    kind,
                    passed: false,
                    message: format!("no successful responses for {}", kind.label()),
                };
            }

            let (passed, message): (bool, &str) = match kind {
                WorktreeKind::Master => (
                    check_all(&relevant, |d| {
                        d.contains(BASE_SIGNATURE) && !d.contains(MUTATED_SIGNATURE)
                    }),
                    "master must show the unmutated base signature with no bleed from worktree A",
                ),
                WorktreeKind::SignatureChange => (
                    check_all(&relevant, |d| {
                        d.contains(MUTATED_SIGNATURE) && !d.contains(BASE_SIGNATURE)
                    }),
                    "worktree A must show only the mutated signature",
                ),
                WorktreeKind::DependencyChange => (
                    check_all(&relevant, |d| {
                        d.contains(BASE_SIGNATURE) && !d.contains(MUTATED_SIGNATURE)
                    }),
                    "worktree B only changes Cargo.toml; the function signature must stay base form",
                ),
                WorktreeKind::UntrackedFile => (
                    check_all(&relevant, |d| d.contains(UNTRACKED_SYMBOL)),
                    "worktree C must resolve the untracked symbol",
                ),
            };

            VerificationResult {
                kind,
                passed,
                message: message.to_string(),
            }
        })
        .collect()
}

/// Runs the full divergent-worktree benchmark: sets up worktrees, fires concurrent workers'
/// queries at `config.remote`, and verifies correctness. Requires at least [`MIN_WORKERS`]
/// concurrent workers to faithfully simulate a real agent fleet.
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

    let setup = setup(config.base_repo.as_deref(), &workdir)?;
    let worktrees = setup.worktrees;

    let start = Instant::now();
    let mut handles = Vec::with_capacity(config.workers);
    for worker_id in 0..config.workers {
        let wt = worktrees[worker_id % worktrees.len()].clone();
        let remote = config.remote;
        let queries = config.queries_per_worker;
        handles.push(tokio::spawn(async move {
            let mut outcomes = Vec::with_capacity(queries);
            for q in 0..queries {
                let client_name = format!("divergent-bench-worker-{worker_id}-{q}");
                let t0 = Instant::now();
                let outcome = match query_once(
                    remote,
                    &wt.root,
                    &wt.query_file,
                    wt.symbol,
                    client_name,
                )
                .await
                {
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
                };
                outcomes.push(outcome);
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

    if config.keep_workdir {
        // Prevent the scratch TempDir from deleting itself on drop so it can be inspected.
        if let Some(dir) = owned_tempdir.take() {
            let _ = dir.keep();
        }
    }

    let verifications = verify(&all_outcomes);
    let total_errors = all_outcomes.iter().filter(|o| !o.ok).count();
    let all_verified = verifications.iter().all(|v| v.passed);
    let all_passed = all_verified && total_errors == 0;

    let mut latencies_us: Vec<u64> = all_outcomes
        .iter()
        .filter(|o| o.ok)
        .map(|o| o.latency.as_micros() as u64)
        .collect();
    let latency = LatencyStats::from_micros(&mut latencies_us);

    let qps = if elapsed.as_secs_f64() > 0.0 {
        all_outcomes.len() as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    Ok(DivergentBenchReport {
        total_queries: all_outcomes.len(),
        total_errors,
        elapsed,
        qps,
        latency,
        verifications,
        all_passed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(kind: WorktreeKind, ok: bool, detail: &str) -> QueryOutcome {
        QueryOutcome {
            worker_id: 0,
            kind,
            latency: Duration::from_millis(1),
            ok,
            detail: detail.to_string(),
        }
    }

    #[test]
    fn verify_passes_with_clean_divergence() {
        let outcomes = vec![
            synthetic(WorktreeKind::Master, true, BASE_SIGNATURE),
            synthetic(WorktreeKind::SignatureChange, true, MUTATED_SIGNATURE),
            synthetic(WorktreeKind::DependencyChange, true, BASE_SIGNATURE),
            synthetic(
                WorktreeKind::UntrackedFile,
                true,
                &format!("pub fn {UNTRACKED_SYMBOL}() -> &'static str"),
            ),
        ];
        let results = verify(&outcomes);
        assert!(results.iter().all(|r| r.passed), "{results:?}");
    }

    #[test]
    fn verify_catches_cross_worktree_bleed_into_master() {
        // Master's response contains worktree A's mutated signature: a routing/bleed bug.
        let outcomes = vec![
            synthetic(WorktreeKind::Master, true, MUTATED_SIGNATURE),
            synthetic(WorktreeKind::SignatureChange, true, MUTATED_SIGNATURE),
            synthetic(WorktreeKind::DependencyChange, true, BASE_SIGNATURE),
            synthetic(
                WorktreeKind::UntrackedFile,
                true,
                &format!("pub fn {UNTRACKED_SYMBOL}() -> &'static str"),
            ),
        ];
        let results = verify(&outcomes);
        let master = results
            .iter()
            .find(|r| r.kind == WorktreeKind::Master)
            .unwrap();
        assert!(!master.passed);
    }

    #[test]
    fn verify_catches_untracked_symbol_not_resolved() {
        let outcomes = vec![
            synthetic(WorktreeKind::Master, true, BASE_SIGNATURE),
            synthetic(WorktreeKind::SignatureChange, true, MUTATED_SIGNATURE),
            synthetic(WorktreeKind::DependencyChange, true, BASE_SIGNATURE),
            synthetic(WorktreeKind::UntrackedFile, true, "no symbol here"),
        ];
        let results = verify(&outcomes);
        let untracked = results
            .iter()
            .find(|r| r.kind == WorktreeKind::UntrackedFile)
            .unwrap();
        assert!(!untracked.passed);
    }

    #[test]
    fn verify_fails_when_no_successful_responses() {
        let outcomes = vec![synthetic(WorktreeKind::Master, false, "connection refused")];
        let results = verify(&outcomes);
        for kind in WorktreeKind::all() {
            let r = results.iter().find(|r| r.kind == kind).unwrap();
            assert!(!r.passed);
        }
    }

    #[test]
    fn locate_symbol_finds_line_and_column() {
        let content = "line one\npub fn compute_signal(input: i64) -> i64 {\n";
        let (line, col) = locate_symbol(content, FIXTURE_SYMBOL).unwrap();
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
    fn locate_symbol_missing_errors() {
        assert!(locate_symbol("nothing to see here", FIXTURE_SYMBOL).is_err());
    }

    #[test]
    fn extract_hover_text_from_plain_value() {
        let result = serde_json::json!({ "contents": { "value": "hello world" } });
        assert_eq!(extract_hover_text(&result), "hello world");
    }

    #[test]
    fn extract_hover_text_from_array() {
        let result = serde_json::json!({ "contents": [ { "value": "a" }, { "value": "b" } ] });
        assert_eq!(extract_hover_text(&result), "a\nb");
    }

    #[test]
    fn setup_creates_four_worktrees_with_expected_mutations() {
        let tmp = tempfile::tempdir().unwrap();
        let setup = setup(None, tmp.path()).expect("setup should succeed");
        assert_eq!(setup.worktrees.len(), 4);

        for wt in &setup.worktrees {
            assert!(wt.root.exists(), "{:?} should exist", wt.root);
            let content = std::fs::read_to_string(&wt.query_file).unwrap();
            match wt.kind {
                WorktreeKind::Master => assert!(content.contains(BASE_SIGNATURE)),
                WorktreeKind::SignatureChange => assert!(content.contains(MUTATED_SIGNATURE)),
                WorktreeKind::DependencyChange => assert!(content.contains(BASE_SIGNATURE)),
                WorktreeKind::UntrackedFile => assert!(content.contains(UNTRACKED_SYMBOL)),
            }
        }

        let dep_cargo_toml = setup
            .worktrees
            .iter()
            .find(|w| w.kind == WorktreeKind::DependencyChange)
            .unwrap()
            .root
            .join(FIXTURE_CRATE_DIR)
            .join("Cargo.toml");
        let manifest = std::fs::read_to_string(dep_cargo_toml).unwrap();
        assert!(manifest.contains("serde"));

        let master_cargo_toml = setup
            .worktrees
            .iter()
            .find(|w| w.kind == WorktreeKind::Master)
            .unwrap()
            .root
            .join(FIXTURE_CRATE_DIR)
            .join("Cargo.toml");
        let master_manifest = std::fs::read_to_string(master_cargo_toml).unwrap();
        assert!(!master_manifest.contains("serde"));
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
