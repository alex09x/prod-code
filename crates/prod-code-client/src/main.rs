//! prod-code client: ultra-thin CLI bridge for editors and AI coding agents over 10G LAN.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use prod_code_client::divergent_bench::{self, DivergentBenchConfig, WorkspaceMode};
use prod_code_mcp::verify::VerifyKind;
use prod_code_protocol::{HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage};
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use url::Url;

#[derive(Parser, Debug)]
#[command(
    name = "prod-code",
    author = "Alex <alex@prod.codes>",
    version,
    about = "Remote Code Intelligence Client"
)]
struct Cli {
    /// Remote gateway address(es): `host:port[,host:port...]`. With several nodes the
    /// workspace is placed on one of them (rendezvous hashing, remembered locally, failover
    /// to the next alive node). Defaults to PROD_CODE_REMOTE or 127.0.0.1:9400.
    #[arg(
        short,
        long,
        env = "PROD_CODE_REMOTE",
        default_value = "127.0.0.1:9400"
    )]
    remote: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run as drop-in Language Server (stdio LSP bridged over 10G TCP).
    Lsp,
    /// Run as Model Context Protocol (MCP) server for AI coding agents.
    Mcp,
    /// Probe remote gateway status and latency.
    Status,
    /// Show every configured gateway node, its status, and where this checkout is placed.
    Cluster,
    /// Push current worktree delta to remote storage over 10G LAN.
    Sync {
        /// Optional subpath to sync (defaults to entire workspace).
        path: Option<PathBuf>,
    },
    /// Jump to symbol definition: prod-code def <file> <line> <col>
    Def { file: PathBuf, line: u32, col: u32 },
    /// Inspect symbol type & docs: prod-code hover <file> <line> <col>
    Hover { file: PathBuf, line: u32, col: u32 },
    /// Find all references to symbol: prod-code refs <file> <line> <col>
    Refs { file: PathBuf, line: u32, col: u32 },
    /// Who calls the function at a position: prod-code callers <file> <line> <col>
    Callers { file: PathBuf, line: u32, col: u32 },
    /// What the function at a position calls: prod-code callees <file> <line> <col>
    Callees { file: PathBuf, line: u32, col: u32 },
    /// Implementations of the trait / interface at a position: prod-code impls <file> <line> <col>
    Impls { file: PathBuf, line: u32, col: u32 },
    /// List document outline symbols: prod-code symbols <file>
    Symbols { file: PathBuf },
    /// Blast radius of the uncommitted changes: changed functions, their callers and the affected tests
    Impact {
        /// Git ref to diff against (default: working tree vs HEAD)
        #[arg(long)]
        base: Option<String>,
        /// Caller levels to follow
        #[arg(long, default_value_t = 4)]
        depth: usize,
        /// Run the affected tests afterwards
        #[arg(long)]
        run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Usage metrics of every node: who queried what, how often, how fast; exec runs; syncs
    Metrics {
        /// Window in seconds (default 24h; 0 = everything the nodes hold in memory)
        #[arg(long, default_value_t = 86_400)]
        since: u64,
        #[arg(long)]
        json: bool,
    },
    /// Run the tests and explain every failure: site, code, callers, what changed
    Diagnose {
        /// Test filter (as for `prod-code test`)
        filter: Option<String>,
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        #[arg(long)]
        json: bool,
    },
    /// Analyzer diagnostics for a file, in memory (no build): prod-code diagnostics <file>
    Diagnostics {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Check a proposed replacement for a file without writing it: prod-code validate <file> --from NEW (or stdin)
    Validate {
        file: PathBuf,
        /// Path of the proposed content; stdin when omitted
        #[arg(long)]
        from: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Unreferenced functions, methods and types across the checkout
    DeadCode {
        /// Also list exported / public symbols nothing in the checkout uses
        #[arg(long)]
        include_exported: bool,
        /// Stop after this many source files
        #[arg(long, default_value_t = 400)]
        max_files: usize,
        #[arg(long)]
        json: bool,
    },
    /// Show a source file that lives on the gateway (std, registry, SDK): prod-code source <path> [--line N] [--context K]
    Source {
        path: String,
        #[arg(long)]
        line: Option<u32>,
        #[arg(long, default_value_t = 20)]
        context: u32,
    },
    /// List code actions (inline, extract, generate, rewrite, quick fixes) at a 1-based
    /// position or selection: prod-code assists <file> <line> <col> [--to LINE:COL]
    Assists {
        file: PathBuf,
        line: u32,
        col: u32,
        /// End of a selection as LINE:COL (1-based).
        #[arg(long)]
        to: Option<String>,
    },
    /// Apply one code action by id at a position or selection and write the edits locally:
    /// prod-code assist <file> <line> <col> <id> [--to LINE:COL] [--subtype N]
    Assist {
        file: PathBuf,
        line: u32,
        col: u32,
        id: String,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        subtype: Option<u64>,
    },
    /// Delete the item at 1-based <line> <col> if nothing references it, else list the usages:
    /// prod-code safe-delete <file> <line> <col>
    SafeDelete { file: PathBuf, line: u32, col: u32 },
    /// Rename the symbol at 1-based <line> <col> across the workspace and apply the edits
    /// locally: prod-code rename <file> <line> <col> <new_name>
    Rename {
        file: PathBuf,
        line: u32,
        col: u32,
        new_name: String,
    },
    /// Compile-check the workspace remotely (cargo check / go build) with structured diagnostics.
    Check {
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        /// Print the full report as JSON instead of text.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Lint the workspace remotely (cargo clippy -D warnings / go vet) with structured findings.
    Lint {
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Run tests remotely (cargo test / go test -json), optionally filtered by name.
    Test {
        /// Test name filter (cargo test TESTNAME / go test -run).
        filter: Option<String>,
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Run a build/test/lint command on the remote gateway inside this checkout's server copy:
    /// prod-code exec -- cargo test -p my-crate
    Exec {
        /// Kill the command after this many seconds (0 = server default, 1 hour).
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        /// Do not copy back files the command changed on the server (formatters, generators,
        /// lockfiles are pulled back by default).
        #[arg(long, default_value_t = false)]
        no_pull: bool,
        /// Command and arguments (put `--` before them).
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Structural search and replace across the workspace: `pattern ==>> replacement`.
    Codemod {
        /// The rule, for example `$a.unwrap() ==>> $a.expect("invariant")`.
        rule: String,
        /// A file used to resolve the paths the pattern mentions.
        #[arg(long)]
        path: Option<String>,
        /// Write the edits into the checkout instead of only reporting them.
        #[arg(long, default_value_t = false)]
        apply: bool,
    },
    /// Find code by what it does: ranked declarations with the doc comment that matched.
    Search {
        /// What the code does, in words.
        query: String,
        /// Hits to return.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Restrict to declarations under this directory.
        #[arg(long)]
        path: Option<String>,
    },
    /// Print only the code a symbol depends on: its declaration plus the items it uses.
    Slice {
        /// Symbol name (`Metrics::record`, `pkg.Func`), or a file with `--line`.
        target: String,
        /// 1-based line, when `target` is a file path.
        #[arg(long)]
        line: Option<u32>,
        /// 1-based column, when `target` is a file path.
        #[arg(long, default_value_t = 1)]
        character: u32,
        /// How many edges to follow from the seed.
        #[arg(long, default_value_t = 2)]
        depth: u32,
        /// Stop once the slice reaches this many bytes.
        #[arg(long, default_value_t = 24576)]
        max_bytes: usize,
    },
    /// Try several hypotheses (sets of proposed file contents) against a command, each in a
    /// private shadow of the server workspace; print every outcome and the winner's diff.
    ShadowRun {
        /// JSON spec: {"hypotheses":[{"name":"h1","edits":[{"path":"src/x.rs","file":"h1-x.rs"}],"delete":["old.rs"]}]}
        /// (`file` is read relative to the spec file; `new_text` inlines the content).
        spec: PathBuf,
        /// Kill a hypothesis after this many seconds (0 = server default, 1 hour).
        #[arg(long, default_value_t = 0)]
        timeout_secs: u64,
        /// Hypotheses to run at once (0 = server default: cores / 8).
        #[arg(long, default_value_t = 0)]
        parallel: usize,
        /// Write the winning hypothesis into the checkout.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Command and arguments (put `--` before them).
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Benchmark throughput and concurrency across workspaces and worktrees.
    Bench {
        /// Target workspace directories (or worktrees). If omitted, uses current working directory.
        #[arg(short, long, num_args = 1..)]
        workspaces: Vec<PathBuf>,
        /// Number of concurrent client worker connections.
        #[arg(short, long, default_value_t = 16)]
        concurrency: usize,
        /// Pipeline depth per connection (in-flight queries sent without waiting).
        #[arg(short, long, default_value_t = 8)]
        depth: usize,
        /// Benchmark duration in seconds.
        #[arg(long, default_value_t = 5)]
        duration_secs: u64,
    },
    /// Multi-worktree divergence and correctness benchmark: forks isolated git worktrees from a
    /// base repo (e.g. BTCR or govcon-intel), applies controlled mutations (signature change,
    /// dependency manifest change, untracked file), then hammers them with concurrent LSP queries
    /// from a simulated agent fleet to assert zero cross-worktree bleed.
    DivergentBench {
        /// Base git repository to fork worktrees from. If omitted, a disposable scratch
        /// repository with a synthetic fixture crate is created instead.
        #[arg(long)]
        base_repo: Option<PathBuf>,
        /// Scratch directory to materialize the origin clone and worktrees in. Defaults to a
        /// temp directory that is cleaned up afterwards.
        #[arg(long)]
        workdir: Option<PathBuf>,
        /// Number of concurrent simulated workers (minimum 10).
        #[arg(long, default_value_t = 12)]
        workers: usize,
        /// Number of queries each worker issues against its assigned worktree.
        #[arg(long, default_value_t = 5)]
        queries_per_worker: usize,
        /// Keep the generated scratch worktrees on disk after the run for inspection.
        #[arg(long, default_value_t = false)]
        keep_workdir: bool,
        /// Server workspace mapping: `shared` coalesces all worktrees onto one server
        /// workspace (production behaviour), `isolated` gives each worktree its own.
        #[arg(long, value_enum, default_value_t = WorkspaceMode::Isolated)]
        mode: WorkspaceMode,
        /// One gateway session per worker (sync/handshake once, then only queries), like a
        /// long-lived MCP agent. Default: fresh connection with pre-flight sync per query.
        #[arg(long, default_value_t = false)]
        persistent: bool,
        /// Persistent mode: drop the connection without a goodbye after this percentage of
        /// queries (simulated agent SIGKILL) and verify the gateway retires the sessions.
        #[arg(long, default_value_t = 0)]
        churn: u8,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let seeds = prod_code_mcp::cluster::parse_remotes(&cli.remote)?;
    // One seed is enough: the rest of the cluster comes from that node's gossip view.
    let remotes = prod_code_mcp::cluster::discover_nodes(&seeds).await;

    let cwd_root = env::current_dir()
        .ok()
        .map(|d| find_workspace_root(&d).unwrap_or(d));
    // Placement follows the origin repository: every worktree lands on the node that holds
    // the origin's copy, so seeding from that copy and the shared cargo target directory
    // work. The workspace name itself stays per worktree (`<repo>--wt-<hash>`).
    let cwd_workspace = cwd_root
        .as_deref()
        .map(|root| {
            let identity = prod_code_mcp::sync::workspace_identity(root);
            identity.base.unwrap_or(identity.name)
        })
        .unwrap_or_default();
    // The engine a query needs is that of the nearest project of the file it names (or of
    // the current directory): a SwiftPM package inside a Rust repository must land on a
    // macOS node even though the repository root is Rust.
    let cwd_engine = cwd_root.as_deref().and_then(|root| {
        let hint = env::args()
            .skip(1)
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .and_then(|p| std::fs::canonicalize(p).ok())
            .or_else(|| env::current_dir().ok())
            .unwrap_or_else(|| root.to_path_buf());
        prod_code_mcp::sync::engine_project(root, &hint).1
    });

    if matches!(cli.command, Some(Commands::Cluster)) {
        return run_cluster(&remotes, &cwd_workspace, cwd_engine).await;
    }

    let remote = prod_code_mcp::cluster::pick_node(&remotes, &cwd_workspace, cwd_engine).await?;

    match cli.command.unwrap_or(Commands::Lsp) {
        Commands::Lsp => run_lsp_bridge(remote).await,
        // `status` is about the node you name, not about where this checkout is placed.
        Commands::Status => run_status_probe(seeds[0]).await,
        Commands::Cluster => run_cluster(&remotes, &cwd_workspace, cwd_engine).await,
        Commands::Metrics { since, json } => run_metrics(&remotes, since, json).await,
        Commands::Mcp => run_mcp_server(remote).await,
        Commands::Sync { path } => run_sync(remote, path).await,
        Commands::Def { file, line, col } => run_definition(remote, &file, line, col).await,
        Commands::Hover { file, line, col } => run_hover(remote, &file, line, col).await,
        Commands::Refs { file, line, col } => run_references(remote, &file, line, col).await,
        Commands::Callers { file, line, col } => {
            run_call_hierarchy(remote, &file, line, col, true).await
        }
        Commands::Callees { file, line, col } => {
            run_call_hierarchy(remote, &file, line, col, false).await
        }
        Commands::Impls { file, line, col } => run_implementations(remote, &file, line, col).await,
        Commands::Symbols { file } => run_symbols(remote, &file).await,
        Commands::Source {
            path,
            line,
            context,
        } => run_source(remote, &path, line, context).await,
        Commands::Impact {
            base,
            depth,
            run,
            json,
        } => run_impact(remote, base.as_deref(), depth, run, json).await,
        Commands::DeadCode {
            include_exported,
            max_files,
            json,
        } => run_dead_code(remote, include_exported, max_files, json).await,
        Commands::Diagnostics { file, json } => run_diagnostics(remote, &file, None, json).await,
        Commands::Diagnose {
            filter,
            timeout_secs,
            json,
        } => run_diagnose(remote, filter.as_deref(), timeout_secs, json).await,
        Commands::Validate { file, from, json } => {
            let text = match from {
                Some(path) => std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.display()))?,
                None => {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                    buf
                }
            };
            run_diagnostics(remote, &file, Some(text), json).await
        }
        Commands::Rename {
            file,
            line,
            col,
            new_name,
        } => run_rename(remote, &file, line, col, &new_name).await,
        Commands::SafeDelete { file, line, col } => run_safe_delete(remote, &file, line, col).await,
        Commands::Assists {
            file,
            line,
            col,
            to,
        } => run_assist(remote, &file, line, col, to.as_deref(), None, None).await,
        Commands::Assist {
            file,
            line,
            col,
            id,
            to,
            subtype,
        } => run_assist(remote, &file, line, col, to.as_deref(), Some(&id), subtype).await,
        Commands::Check { timeout_secs, json } => {
            run_verify(remote, VerifyKind::Check, None, timeout_secs, json).await
        }
        Commands::Lint { timeout_secs, json } => {
            run_verify(remote, VerifyKind::Lint, None, timeout_secs, json).await
        }
        Commands::Test {
            filter,
            timeout_secs,
            json,
        } => run_verify(remote, VerifyKind::Test, filter, timeout_secs, json).await,
        Commands::Exec {
            timeout_secs,
            no_pull,
            command,
        } => run_exec(remote, command, timeout_secs, !no_pull).await,
        Commands::Codemod { rule, path, apply } => run_codemod_cli(remote, rule, path, apply).await,
        Commands::Search { query, limit, path } => run_search_cli(remote, query, limit, path).await,
        Commands::Slice {
            target,
            line,
            character,
            depth,
            max_bytes,
        } => run_slice(remote, target, line, character, depth, max_bytes).await,
        Commands::ShadowRun {
            spec,
            timeout_secs,
            parallel,
            apply,
            command,
        } => run_shadow_cli(remote, spec, timeout_secs, parallel, apply, command).await,
        Commands::Bench {
            workspaces,
            concurrency,
            depth,
            duration_secs,
        } => run_benchmark(remote, workspaces, concurrency, depth, duration_secs).await,
        Commands::DivergentBench {
            base_repo,
            workdir,
            workers,
            queries_per_worker,
            keep_workdir,
            mode,
            persistent,
            churn,
        } => {
            run_divergent_bench(DivergentBenchConfig {
                remote,
                base_repo,
                workdir,
                workers,
                queries_per_worker,
                keep_workdir,
                mode,
                persistent,
                churn_percent: churn,
            })
            .await
        }
    }
}

/// Dynamically detect the base repository name if current directory is a git worktree or repository.
pub fn detect_workspace_name(dir: &Path) -> Option<String> {
    // Every git worktree gets its own isolated server workspace; see
    // `prod_code_mcp::sync::workspace_identity`.
    Some(prod_code_mcp::sync::workspace_identity(dir).name)
}

/// Find the root of the workspace or git worktree containing the specified file.
pub fn find_workspace_root(file_path: &Path) -> Option<PathBuf> {
    let abs_path = if file_path.is_absolute() {
        std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf())
    } else if let Ok(cwd) = env::current_dir() {
        let joined = cwd.join(file_path);
        std::fs::canonicalize(&joined).unwrap_or(joined)
    } else {
        file_path.to_path_buf()
    };

    let mut current = if abs_path.is_file() {
        abs_path.parent()?
    } else {
        abs_path.as_path()
    };

    let mut candidate_manifest = None;

    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        if candidate_manifest.is_none()
            && (current.join("Cargo.toml").exists()
                || current.join("go.mod").exists()
                || current.join("package.json").exists()
                || current.join("pyproject.toml").exists())
        {
            candidate_manifest = Some(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }

    candidate_manifest
}

/// Helper to connect, initialize, and execute a targeted LSP request against the remote gateway.
/// Phase timings for one query, printed to stderr when `PROD_CODE_TIMING=1`.
struct QueryTiming {
    enabled: bool,
    start: std::time::Instant,
    last: std::time::Instant,
    phases: Vec<(&'static str, f64)>,
}

impl QueryTiming {
    fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            enabled: env::var_os("PROD_CODE_TIMING").is_some(),
            start: now,
            last: now,
            phases: Vec::new(),
        }
    }

    fn mark(&mut self, phase: &'static str) {
        if !self.enabled {
            return;
        }
        let now = std::time::Instant::now();
        self.phases
            .push((phase, (now - self.last).as_secs_f64() * 1000.0));
        self.last = now;
    }

    fn report(&self) {
        if !self.enabled {
            return;
        }
        let total = self.start.elapsed().as_secs_f64() * 1000.0;
        let parts: Vec<String> = self
            .phases
            .iter()
            .map(|(name, ms)| format!("{name}={ms:.1}ms"))
            .collect();
        eprintln!("[timing] total={total:.1}ms {}", parts.join(" "));
    }
}

async fn execute_lsp_query(
    remote: SocketAddr,
    file_path: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let cwd = env::current_dir().context("Failed to determine current working directory")?;

    let abs_path = if file_path.is_absolute() {
        std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf())
    } else {
        let joined = cwd.join(file_path);
        std::fs::canonicalize(&joined).unwrap_or(joined)
    };

    let ws_root = find_workspace_root(&abs_path).unwrap_or_else(|| cwd.clone());
    let ws_root_str = ws_root.to_string_lossy().to_string();
    let base_ws_name = detect_workspace_name(&ws_root);

    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", abs_path))?
        .to_string();

    let file_content = tokio::fs::read_to_string(&abs_path)
        .await
        .with_context(|| format!("Failed to read file {:?}", abs_path))?;

    let mut timing = QueryTiming::new();
    let mut framed = {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let stream = TcpStream::connect(remote)
                .await
                .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
            let _ = stream.set_nodelay(true);
            let mut framed = Framed::new(stream, ProdCodeCodec::new());
            timing.mark("connect");

            // 1. Handshake
            // 1a. Transparent pre-flight sync before the handshake: manifest probe on first
            // contact (seeded from the origin repository's copy), watermark delta afterwards.
            let identity = prod_code_mcp::sync::workspace_identity(&ws_root);
            if let Err(e) =
                prod_code_mcp::sync::push_workspace_sync(&mut framed, &ws_root, &identity, None)
                    .await
            {
                tracing::warn!(error = %e, "pre-flight workspace sync failed");
            }
            timing.mark("preflight_sync");

            let (engine_subpath, expected_engine) =
                prod_code_mcp::sync::engine_project(&ws_root, &abs_path);
            framed
                .send(WireMessage::HandshakeRequest(HandshakeRequest {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "prod-code-cli".to_string(),
                    client_pid: std::process::id(),
                    auth_token: None,
                    client_workspace_root: ws_root_str.clone(),
                    preferred_engine: None,
                    base_workspace_name: base_ws_name.clone(),
                    engine_subpath,
                    client_agent: Some(prod_code_protocol::detect_client_agent()),
                    client_host: Some(prod_code_protocol::client_host()),
                }))
                .await?;

            let handshake = match framed.next().await {
                Some(Ok(WireMessage::HandshakeResponse(resp))) => resp,
                other => anyhow::bail!("Unexpected handshake response: {:?}", other),
            };

            // Self-heal: the gateway keyed this workspace on an empty or reset directory (its
            // detected engine does not match our manifest) while our watermark still claims
            // everything was sent. Forget the watermark, push the full tree and reconnect;
            // the gateway reloads a workspace whose engine kind changed.
            if attempt == 1
                && let Some(expected) = expected_engine
                && handshake.detected_engine != expected
            {
                prod_code_mcp::sync::clear_sync_cache(&ws_root);
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: "engine mismatch; resyncing workspace".to_string(),
                    })
                    .await;
                continue;
            }
            timing.mark("handshake");
            break framed;
        }
    };

    // 2. LSP Initialize
    let folder_name = ws_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": format!("file://{}", ws_root.to_string_lossy()),
            "workspaceFolders": [
                {
                    "name": folder_name,
                    "uri": format!("file://{}", ws_root.to_string_lossy())
                }
            ],
            "capabilities": {
                "workspace": {
                    "workspaceFolders": true,
                    "configuration": true
                },
                "textDocument": {
                    "hover": {
                        "contentFormat": ["markdown", "plaintext"]
                    },
                    "definition": {
                        "linkSupport": true
                    },
                    "documentSymbol": {
                        "hierarchicalDocumentSymbolSupport": true
                    },
                    "references": {}
                }
            }
        }
    });
    framed
        .send(WireMessage::LspPayload(init_req.to_string()))
        .await?;

    // Await init response (matching id: 1)
    let init_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
    while tokio::time::Instant::now() < init_deadline {
        let remaining = init_deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(resp_json)))) => {
                if serde_json::from_str::<serde_json::Value>(&resp_json)
                    .ok()
                    .and_then(|val| val.get("id").and_then(|id| id.as_i64()))
                    == Some(1)
                {
                    break;
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => anyhow::bail!("Frame decode error during initialize: {}", e),
            Ok(None) => anyhow::bail!("Server closed connection during initialize"),
            Err(_) => anyhow::bail!("Timeout waiting for initialize response"),
        }
    }

    // 3. LSP Initialized notification
    timing.mark("initialize");
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    framed
        .send(WireMessage::LspPayload(initialized.to_string()))
        .await?;

    // 4. LSP didOpen notification
    let language_id = prod_code_mcp::lang::language_id_for_path(&abs_path);
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": file_uri,
                "languageId": language_id,
                "version": 1,
                "text": file_content
            }
        }
    });
    framed
        .send(WireMessage::LspPayload(did_open.to_string()))
        .await?;
    timing.mark("did_open_sent");

    // 5. Send targeted query with id = 2
    let query_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": method,
        "params": params
    });
    framed
        .send(WireMessage::LspPayload(query_req.to_string()))
        .await?;

    // 6. Read response matching id = 2
    let query_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(60);
    while tokio::time::Instant::now() < query_deadline {
        let remaining = query_deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(resp_json)))) => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&resp_json)
                    .map_err(|_| ())
                    .and_then(|v| {
                        if v.get("id").and_then(|id| id.as_i64()) == Some(2) {
                            Ok(v)
                        } else {
                            Err(())
                        }
                    })
                {
                    timing.mark("query_response");
                    timing.report();
                    let did_close = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/didClose",
                        "params": {
                            "textDocument": {
                                "uri": file_uri
                            }
                        }
                    });
                    let _ = framed
                        .send(WireMessage::LspPayload(did_close.to_string()))
                        .await;
                    let _ = framed
                        .send(WireMessage::Disconnect {
                            reason: "query finished".to_string(),
                        })
                        .await;
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
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => anyhow::bail!("Frame decode error: {}", e),
            Ok(None) => anyhow::bail!("Remote closed connection prematurely"),
            Err(_) => anyhow::bail!("Timeout waiting for LSP response to {}", method),
        }
    }

    anyhow::bail!("No response received for query {}", method)
}

async fn run_hover(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();

    let lsp_line = line.saturating_sub(1);
    let lsp_col = col.saturating_sub(1);

    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": lsp_line, "character": lsp_col }
    });

    let result = execute_lsp_query(remote, file, "textDocument/hover", params).await?;

    if let Some(contents) = result.get("contents") {
        if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
            println!("{value}");
            return Ok(());
        } else if let Some(arr) = contents.as_array() {
            for item in arr {
                if let Some(v) = item.get("value").and_then(|v| v.as_str()) {
                    println!("{v}");
                }
            }
            return Ok(());
        }
    }

    println!("{:#}", result);
    Ok(())
}

async fn run_definition(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();

    let lsp_line = line.saturating_sub(1);
    let lsp_col = col.saturating_sub(1);

    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": lsp_line, "character": lsp_col }
    });

    let result = execute_lsp_query(remote, file, "textDocument/definition", params).await?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or_else(|| abs_path.clone());
    let mut shown = 0;

    if let Some(arr) = result.as_array() {
        if arr.is_empty() {
            println!("No definition found.");
        } else {
            for loc in arr {
                let uri = loc
                    .get("uri")
                    .or_else(|| loc.get("targetUri"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("");
                let range = loc.get("range").or_else(|| loc.get("targetSelectionRange"));
                let start_line = range
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
                    + 1;
                let start_col = range
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    + 1;
                println!("📍 Definition: {uri}:{start_line}:{start_col}");
                // A definition outside the checkout lives only on the gateway: show it.
                let path = prod_code_mcp::remote_fs::uri_to_path(uri);
                if prod_code_mcp::remote_fs::is_external(&ws_root, &path) && shown < 3 {
                    shown += 1;
                    match prod_code_mcp::remote_fs::read_remote_file(remote, &path, 0).await {
                        Ok((bytes, _)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            print!(
                                "{}",
                                prod_code_mcp::remote_fs::snippet(&text, start_line as u32, 8)
                            );
                        }
                        Err(e) => println!("   (external source not readable: {e})"),
                    }
                }
            }
        }
    } else if let Some(obj) = result.as_object() {
        let uri = obj.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let start_line = obj
            .get("range")
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0)
            + 1;
        let start_col = obj
            .get("range")
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0)
            + 1;
        println!("📍 Definition: {uri}:{start_line}:{start_col}");
    } else {
        println!("{:#}", result);
    }

    Ok(())
}

/// Impact analysis of the working tree (or of the commits since `base`), optionally running
/// the affected tests on the gateway.
async fn run_impact(
    remote: SocketAddr,
    base: Option<&str>,
    depth: usize,
    run: bool,
    json: bool,
) -> Result<()> {
    use std::io::Write;
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let started = std::time::Instant::now();
    let report = prod_code_mcp::impact::analyze(remote, &root, base, depth).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
        eprintln!(
            "[prod-code impact] analysed in {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }
    if run {
        let Some(command) = report.test_command.clone() else {
            println!("nothing to run");
            return Ok(());
        };
        println!("$ {}", command.join(" "));
        let outcome = prod_code_mcp::exec::run_remote(
            remote,
            &root,
            None,
            command,
            vec![("CARGO_TERM_COLOR".to_string(), "never".to_string())],
            0,
            false,
            |is_stderr, data| {
                if is_stderr {
                    let _ = std::io::stderr().write_all(data);
                } else {
                    let _ = std::io::stdout().write_all(data);
                }
            },
        )
        .await?;
        std::process::exit(outcome.exit.exit_code.unwrap_or(1));
    }
    Ok(())
}

/// Usage metrics merged across every node of the cluster.
async fn run_metrics(nodes: &[SocketAddr], since: u64, json: bool) -> Result<()> {
    let mut all = Vec::new();
    for node in nodes {
        match prod_code_mcp::cluster::node_metrics(*node, since).await {
            Ok(m) => all.push(m),
            Err(e) => eprintln!("{node}: {e}"),
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(());
    }
    let window = if since == 0 {
        "all in memory".to_string()
    } else {
        format!("last {}h{:02}m", since / 3600, (since % 3600) / 60)
    };
    println!("⚡ prod-code usage ({window}, {} node(s))", all.len());
    println!("────────────────────────────────────────────────────────────────────────");
    // Queries: by agent → workspace → method, summed over nodes.
    type Key = (String, String, String, String);
    type Agg = (u64, u64, u64, u64);
    let mut by_key: std::collections::BTreeMap<Key, Agg> = std::collections::BTreeMap::new();
    for m in &all {
        for q in &m.queries {
            let e = by_key
                .entry((
                    q.agent.clone(),
                    q.host.clone(),
                    q.workspace.clone(),
                    q.method.clone(),
                ))
                .or_default();
            e.0 += q.count;
            e.1 += q.errors;
            e.2 = e.2.max(q.p50_ms);
            e.3 = e.3.max(q.p95_ms);
        }
    }
    if by_key.is_empty() {
        println!("no queries in the window");
    } else {
        println!(
            "{:<12} {:<18} {:<28} {:<34} {:>7} {:>5} {:>7} {:>7}",
            "agent", "host", "workspace", "method", "count", "err", "p50ms", "p95ms"
        );
        for ((agent, host, ws, method), (count, errors, p50, p95)) in &by_key {
            println!(
                "{:<12} {:<18} {:<28} {:<34} {:>7} {:>5} {:>7} {:>7}",
                truncate(agent, 12),
                truncate(host, 18),
                truncate(ws, 28),
                truncate(method.trim_start_matches("textDocument/"), 34),
                count,
                errors,
                p50,
                p95
            );
        }
    }
    let mut execs: Vec<_> = all.iter().flat_map(|m| m.execs.iter().cloned()).collect();
    if !execs.is_empty() {
        execs.sort_by_key(|e| std::cmp::Reverse(e.total_ms));
        println!("────────────────────────────────────────────────────────────────────────");
        println!(
            "{:<12} {:<18} {:<28} {:<40} {:>5} {:>4} {:>8}",
            "agent", "host", "workspace", "command", "runs", "fail", "total s"
        );
        for e in execs.iter().take(30) {
            println!(
                "{:<12} {:<18} {:<28} {:<40} {:>5} {:>4} {:>8.1}",
                truncate(&e.agent, 12),
                truncate(&e.host, 18),
                truncate(&e.workspace, 28),
                truncate(&e.command, 40),
                e.count,
                e.failures,
                e.total_ms as f64 / 1000.0
            );
        }
    }
    println!("────────────────────────────────────────────────────────────────────────");
    for m in &all {
        println!(
            "{:<22} syncs {:>5}  files {:>7}  {:>8.1} MB  events in memory {}",
            m.node,
            m.sync_rounds,
            m.sync_files,
            m.sync_bytes as f64 / 1_048_576.0,
            m.events_in_memory
        );
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Runs the tests and prints a dossier for every failure.
async fn run_diagnose(
    remote: SocketAddr,
    filter: Option<&str>,
    timeout_secs: u64,
    json: bool,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let report = prod_code_mcp::dossier::diagnose(remote, &root, filter, timeout_secs).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
    }
    if report.tests_failed > 0 || !report.build_errors.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

/// Diagnostics of a file as it is, or as it would be with `proposed` content (nothing is
/// written). Exits 1 when there are errors.
async fn run_diagnostics(
    remote: SocketAddr,
    file: &Path,
    proposed: Option<String>,
    json: bool,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let started = std::time::Instant::now();
    let report = match proposed {
        Some(text) => {
            prod_code_mcp::diagnostics::validate_text(remote, &root, &abs_path, &text).await?
        }
        None => prod_code_mcp::diagnostics::diagnostics(remote, &root, &abs_path).await?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
        eprintln!(
            "[prod-code] analysed in {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }
    if !report.ok() {
        std::process::exit(1);
    }
    Ok(())
}

/// Scans the checkout for unreferenced symbols.
async fn run_dead_code(
    remote: SocketAddr,
    include_exported: bool,
    max_files: usize,
    json: bool,
) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let started = std::time::Instant::now();
    let report =
        prod_code_mcp::dead_code::find_dead_code(remote, &root, include_exported, max_files)
            .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render());
        eprintln!(
            "[prod-code dead-code] scanned in {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

/// Prints a source file that lives on the gateway host (toolchain sources, dependency caches,
/// SDK headers), optionally a window around one line.
async fn run_source(remote: SocketAddr, path: &str, line: Option<u32>, context: u32) -> Result<()> {
    let path = prod_code_mcp::remote_fs::uri_to_path(path);
    let (bytes, truncated) = prod_code_mcp::remote_fs::read_remote_file(remote, &path, 0).await?;
    let text = String::from_utf8_lossy(&bytes);
    match line {
        Some(line) => print!(
            "{}",
            prod_code_mcp::remote_fs::snippet(&text, line, context)
        ),
        None => print!("{text}"),
    }
    if truncated {
        eprintln!("[prod-code] {path}: output truncated at 2 MiB");
    }
    Ok(())
}

/// Prints `uri:line:col` for every LSP `Location` in `arr`.
fn print_locations(arr: &[serde_json::Value]) {
    for loc in arr {
        let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let start = loc.get("range").and_then(|r| r.get("start"));
        let line = start
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0)
            + 1;
        let col = start
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0)
            + 1;
        println!("  • {uri}:{line}:{col}");
    }
}

/// Callers (`incoming`) or callees of the function at a 1-based position: the call-hierarchy
/// item is prepared first, then its incoming or outgoing calls are listed with call sites.
async fn run_call_hierarchy(
    remote: SocketAddr,
    file: &Path,
    line: u32,
    col: u32,
    incoming: bool,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
    });
    let items =
        execute_lsp_query(remote, file, "textDocument/prepareCallHierarchy", params).await?;
    let Some(item) = items.as_array().and_then(|a| a.first()).cloned() else {
        println!("No function at {}:{line}:{col}.", file.display());
        return Ok(());
    };
    let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("?");
    let method = if incoming {
        "callHierarchy/incomingCalls"
    } else {
        "callHierarchy/outgoingCalls"
    };
    let result =
        execute_lsp_query(remote, file, method, serde_json::json!({ "item": item })).await?;
    let edges = result.as_array().cloned().unwrap_or_default();
    let side = if incoming { "from" } else { "to" };
    if edges.is_empty() {
        println!(
            "`{name}`: no {} found.",
            if incoming { "callers" } else { "callees" }
        );
        return Ok(());
    }
    println!(
        "`{name}`: {} {}",
        edges.len(),
        if incoming { "caller(s)" } else { "callee(s)" }
    );
    for edge in &edges {
        let other = edge.get(side).cloned().unwrap_or_default();
        let other_name = other.get("name").and_then(|n| n.as_str()).unwrap_or("?");
        let uri = other.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let start = other.get("selectionRange").and_then(|r| r.get("start"));
        let dl = start
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0)
            + 1;
        let dc = start
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0)
            + 1;
        let sites: Vec<String> = edge
            .get("fromRanges")
            .and_then(|r| r.as_array())
            .map(|ranges| {
                ranges
                    .iter()
                    .filter_map(|r| r.get("start"))
                    .map(|s| {
                        format!(
                            "{}:{}",
                            s.get("line").and_then(|l| l.as_u64()).unwrap_or(0) + 1,
                            s.get("character").and_then(|c| c.as_u64()).unwrap_or(0) + 1
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        println!(
            "  • {other_name}  {uri}:{dl}:{dc}  [call sites: {}]",
            sites.join(", ")
        );
    }
    Ok(())
}

async fn run_implementations(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
    });
    let result = execute_lsp_query(remote, file, "textDocument/implementation", params).await?;
    let arr = match &result {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(_) => vec![result.clone()],
        _ => Vec::new(),
    };
    if arr.is_empty() {
        println!("No implementations found.");
    } else {
        println!("Found {} implementation(s):", arr.len());
        print_locations(&arr);
    }
    Ok(())
}

async fn run_references(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();

    let lsp_line = line.saturating_sub(1);
    let lsp_col = col.saturating_sub(1);

    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": lsp_line, "character": lsp_col },
        "context": { "includeDeclaration": false }
    });

    let result = execute_lsp_query(remote, file, "textDocument/references", params).await?;

    if let Some(arr) = result.as_array() {
        if arr.is_empty() {
            println!("No references found.");
        } else {
            println!("Found {} reference(s):", arr.len());
            for loc in arr {
                let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                let start_line = loc
                    .get("range")
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
                    + 1;
                let start_col = loc
                    .get("range")
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    + 1;
                println!("  • {uri}:{start_line}:{start_col}");
            }
        }
    } else {
        println!("{:#}", result);
    }

    Ok(())
}

async fn run_symbols(remote: SocketAddr, file: &Path) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();

    let params = serde_json::json!({
        "textDocument": { "uri": file_uri }
    });

    let result = execute_lsp_query(remote, file, "textDocument/documentSymbol", params).await?;

    if let Some(arr) = result.as_array() {
        println!("Symbols in {:?}:", file);
        for sym in arr {
            let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
            let kind_str = match kind {
                2 => "Module",
                5 => "Class",
                6 => "Method",
                8 => "Field",
                9 => "Constructor",
                10 => "Enum",
                11 => "Interface",
                12 => "Function",
                13 => "Variable",
                14 => "Constant",
                22 => "EnumMember",
                23 => "Struct",
                _ => "Symbol",
            };
            // DocumentSymbol carries `range`; SymbolInformation nests it under `location`.
            let start_line = sym
                .get("range")
                .or_else(|| sym.get("location").and_then(|l| l.get("range")))
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_u64())
                .unwrap_or(0)
                + 1;
            println!("  [{kind_str}] {name} (line {start_line})");
        }
    } else {
        println!("{:#}", result);
    }

    Ok(())
}

/// Query remote gateway for health and status snapshot.
async fn run_status_probe(remote: SocketAddr) -> Result<()> {
    let start = std::time::Instant::now();
    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to prod-code gateway at {remote}"))?;
    let _ = stream.set_nodelay(true);
    let rtt = start.elapsed();

    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed.send(WireMessage::StatusRequest).await?;

    if let Some(msg) = framed.next().await {
        match msg? {
            WireMessage::StatusResponse(resp) => {
                let hours = resp.uptime_seconds / 3600;
                let minutes = (resp.uptime_seconds % 3600) / 60;
                let seconds = resp.uptime_seconds % 60;

                println!("⚡ prod-code Remote Code Intelligence Gateway");
                println!("────────────────────────────────────────────────────");
                println!("Remote Address:    {remote} ({:.2?} RTT)", rtt);
                println!("Server PID:        {}", resp.server_pid);
                println!("Uptime:            {}h {}m {}s", hours, minutes, seconds);
                if let Some(mb) = resp.memory_rss_mb() {
                    println!("Memory RSS:        {:.2} MB", mb);
                }
                println!("Active Sessions:   {}", resp.active_sessions);
                println!("Loaded Workspaces: {}", resp.loaded_workspaces);
                println!(
                    "Queries Handled:   {} (in-flight: {})",
                    resp.total_queries, resp.active_queries
                );
                println!("Engines Available: {}", resp.detected_engines.join(", "));
                println!("Status:            HEALTHY");
            }
            other => anyhow::bail!("Unexpected response from gateway: {:?}", other),
        }
    } else {
        anyhow::bail!("Gateway closed connection without responding");
    }

    Ok(())
}

/// Run full-duplex stdio LSP bridge connecting local editor to remote daemon over TCP.
async fn run_lsp_bridge(remote: SocketAddr) -> Result<()> {
    let cwd = env::current_dir().context("Failed to determine current working directory")?;
    let cwd_str = cwd.to_string_lossy().to_string();

    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
    let _ = stream.set_nodelay(true);
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    // Perform handshake
    framed
        .send(WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: PROTOCOL_VERSION,
            client_name: "prod-code-client".to_string(),
            client_pid: std::process::id(),
            auth_token: None,
            client_workspace_root: cwd_str,
            preferred_engine: None,
            base_workspace_name: detect_workspace_name(&cwd),
            engine_subpath: None,
            client_agent: Some(prod_code_protocol::detect_client_agent()),
            client_host: Some(prod_code_protocol::client_host()),
        }))
        .await?;

    let handshake_resp = match framed.next().await {
        Some(Ok(WireMessage::HandshakeResponse(resp))) => resp,
        Some(Ok(other)) => anyhow::bail!("Expected HandshakeResponse, got {:?}", other),
        Some(Err(err)) => return Err(err.into()),
        None => anyhow::bail!("Server closed connection during handshake"),
    };

    tracing::debug!(
        session_id = handshake_resp.session_id,
        engine = handshake_resp.detected_engine,
        "Connected to remote gateway"
    );

    let (mut socket_tx, mut socket_rx) = framed.split();

    // Spawn background task to read responses from server and write LSP to stdout
    let stdout_task = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(msg_res) = socket_rx.next().await {
            match msg_res {
                Ok(WireMessage::LspPayload(json)) => {
                    let header = format!("Content-Length: {}\r\n\r\n", json.len());
                    if stdout.write_all(header.as_bytes()).await.is_err() {
                        break;
                    }
                    if stdout.write_all(json.as_bytes()).await.is_err() {
                        break;
                    }
                    if stdout.flush().await.is_err() {
                        break;
                    }
                }
                Ok(WireMessage::Disconnect { .. }) => break,
                Err(_) => break,
                _ => {}
            }
        }
    });

    // Main loop: read standard LSP from stdin and forward as WireMessage::LspPayload over TCP
    let mut stdin_reader = BufReader::new(tokio::io::stdin());
    let mut header_line = String::new();

    loop {
        header_line.clear();
        let bytes_read = stdin_reader.read_line(&mut header_line).await?;
        if bytes_read == 0 {
            // Stdin EOF (editor exited)
            let _ = socket_tx
                .send(WireMessage::Disconnect {
                    reason: "stdin EOF".to_string(),
                })
                .await;
            break;
        }

        // Parse Content-Length header
        if header_line.starts_with("Content-Length:") {
            let len_str = header_line.trim_start_matches("Content-Length:").trim();
            let content_len: usize = len_str.parse().context("Invalid Content-Length header")?;

            // Read the empty separating line \r\n
            header_line.clear();
            stdin_reader.read_line(&mut header_line).await?;

            // Read exact body
            let mut body_buf = vec![0u8; content_len];
            stdin_reader.read_exact(&mut body_buf).await?;

            let json_payload =
                String::from_utf8(body_buf).context("LSP payload was not valid UTF-8 string")?;

            if socket_tx
                .send(WireMessage::LspPayload(json_payload))
                .await
                .is_err()
            {
                break;
            }
        }
    }

    let _ = stdout_task.await;
    Ok(())
}

async fn run_mcp_server(remote: SocketAddr) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    prod_code_mcp::run_stdio_mcp_server(remote, cwd).await
}

async fn run_sync(remote: SocketAddr, subpath: Option<PathBuf>) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let start = std::time::Instant::now();
    let identity = prod_code_mcp::sync::workspace_identity(&cwd);

    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
    let _ = stream.set_nodelay(true);
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    let outcome =
        prod_code_mcp::sync::push_workspace_sync(&mut framed, &cwd, &identity, subpath.as_deref())
            .await?;
    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "sync finished".to_string(),
        })
        .await;

    let total_ms = start.elapsed().as_millis();
    let kb = (outcome.bytes_transferred as f64) / 1024.0;
    println!("⚡ prod-code Fast-Sync Completed in {total_ms}ms");
    println!("────────────────────────────────────────────────────");
    println!("Local Workspace:   {}", cwd.display());
    println!("Server Workspace:  {}", identity.name);
    if !outcome.server_workspace_root.is_empty() {
        println!("Remote Path:       {}", outcome.server_workspace_root);
    }
    println!("Files Planned:     {}", outcome.planned);
    if outcome.probed {
        println!(
            "Manifest Probe:    {} files already on server{}",
            outcome.planned.saturating_sub(outcome.files_updated),
            if outcome.seeded {
                " (seeded from origin copy)"
            } else {
                ""
            }
        );
    }
    println!("Files Updated:     {}", outcome.files_updated);
    println!("Files Deleted:     {}", outcome.files_deleted);
    println!("Data Transferred:  {kb:.1} KB");
    println!("Status:            SYNCHRONIZED");
    Ok(())
}

fn find_first_code_file(dir: &Path) -> Option<(PathBuf, u32, u32)> {
    let mut builder = ignore::WalkBuilder::new(dir);
    builder.hidden(true).git_ignore(true).max_depth(Some(4));

    for entry in builder.build().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "rs" | "go" | "py" | "ts") || path.to_string_lossy().contains("/tests/") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };

        for (line_idx, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//")
                || trimmed.starts_with("/*")
                || trimmed.starts_with("#")
                || trimmed.is_empty()
            {
                continue;
            }
            if let Some(pos) = trimmed.find("pub const ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 12) as u32));
            }
            if let Some(pos) = trimmed.find("pub struct ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 13) as u32));
            }
            if let Some(pos) = trimmed.find("pub fn ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 9) as u32));
            }
            if let Some(pos) = trimmed.find("func ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 6) as u32));
            }
        }
    }
    None
}

/// Delete an unreferenced item through the remote analyzer and apply the edit locally.
async fn run_safe_delete(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) }
    });
    let started = std::time::Instant::now();
    let edit = execute_lsp_query(remote, file, "prodCode/safeDelete", params).await?;
    if edit.is_null() {
        anyhow::bail!("safe delete produced no edits");
    }
    let touched = prod_code_mcp::refactor::apply_workspace_edit(&ws_root, &edit)?;
    println!(
        "deleted in {:.2}s; {} path(s) updated:",
        started.elapsed().as_secs_f64(),
        touched.len()
    );
    for path in touched {
        println!("  {path}");
    }
    Ok(())
}

fn parse_line_col(spec: &str) -> Result<(u32, u32)> {
    let (l, c) = spec
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("expected LINE:COL, got {spec}"))?;
    Ok((l.trim().parse()?, c.trim().parse()?))
}

/// List code actions at a position (no `id`) or apply one (`id`) and write its edits locally.
async fn run_assist(
    remote: SocketAddr,
    file: &Path,
    line: u32,
    col: u32,
    to: Option<&str>,
    id: Option<&str>,
    subtype: Option<u64>,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();
    let end = match to {
        Some(spec) => parse_line_col(spec)?,
        None => (line, col),
    };
    let mut params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "range": {
            "start": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            "end": { "line": end.0.saturating_sub(1), "character": end.1.saturating_sub(1) }
        }
    });
    match id {
        None => {
            let list = execute_lsp_query(remote, file, "prodCode/assists", params).await?;
            let items = list.as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                println!("no code actions at {}:{line}:{col}", file.display());
            }
            for item in items {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
                match item.get("subtype").and_then(|v| v.as_u64()) {
                    Some(st) => println!("{id} --subtype {st}  [{kind}]  {label}"),
                    None => println!("{id}  [{kind}]  {label}"),
                }
            }
            Ok(())
        }
        Some(id) => {
            params["id"] = serde_json::json!(id);
            if let Some(st) = subtype {
                params["subtype"] = serde_json::json!(st);
            }
            let started = std::time::Instant::now();
            let edit = execute_lsp_query(remote, file, "prodCode/applyAssist", params).await?;
            if edit.is_null() {
                anyhow::bail!("assist produced no edits");
            }
            let touched = prod_code_mcp::refactor::apply_workspace_edit(&ws_root, &edit)?;
            println!(
                "applied `{id}` in {:.2}s; {} path(s) updated:",
                started.elapsed().as_secs_f64(),
                touched.len()
            );
            for path in touched {
                println!("  {path}");
            }
            Ok(())
        }
    }
}

/// Rename a symbol through the remote analyzer and apply the resulting edits to the checkout.
async fn run_rename(
    remote: SocketAddr,
    file: &Path,
    line: u32,
    col: u32,
    new_name: &str,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        "newName": new_name
    });
    let started = std::time::Instant::now();
    let edit = execute_lsp_query(remote, file, "textDocument/rename", params).await?;
    if edit.is_null() {
        anyhow::bail!("rename produced no edits");
    }
    let touched = prod_code_mcp::refactor::apply_workspace_edit(&ws_root, &edit)?;
    println!(
        "renamed to `{new_name}` in {:.2}s; {} path(s) updated:",
        started.elapsed().as_secs_f64(),
        touched.len()
    );
    for path in touched {
        println!("  {path}");
    }
    Ok(())
}

/// Show every gateway node and the placement of the current checkout.
async fn run_cluster(
    nodes: &[SocketAddr],
    workspace_name: &str,
    engine: Option<&str>,
) -> Result<()> {
    println!("⚡ prod-code cluster ({} node(s))", nodes.len());
    println!("────────────────────────────────────────────────────");
    // The gossip view of the first node that answers: what every node holds.
    for node in nodes {
        if let Ok(view) = prod_code_mcp::cluster::cluster_view(*node).await {
            println!("gossip view from {}:", view.this_node);
            for peer in &view.nodes {
                let ws: Vec<String> = peer
                    .workspaces
                    .iter()
                    .map(|w| format!("{}[{}:{}]", w.name, w.engine, w.sessions))
                    .collect();
                println!(
                    "  {:<22} {:<5} load {:>5.2}/cpu  seen {:>3}s ago  {}",
                    peer.addr,
                    if peer.alive { "UP" } else { "STALE" },
                    peer.status.load_per_cpu().unwrap_or(0.0),
                    peer.last_seen_secs,
                    if ws.is_empty() {
                        "-".to_string()
                    } else {
                        ws.join(" ")
                    }
                );
            }
            println!("────────────────────────────────────────────────────");
            break;
        }
    }
    let home = prod_code_mcp::cluster::rendezvous_order(nodes, workspace_name)
        .first()
        .copied();
    let remembered = prod_code_mcp::cluster::remembered_node(workspace_name);
    for node in nodes {
        let started = std::time::Instant::now();
        match prod_code_mcp::cluster::node_status(*node).await {
            Ok(status) => {
                println!(
                    "{node:<22} UP    {:>6.2} ms  load {:>5.2}/cpu ({} cpus)  uptime {}h{:02}m  workspaces {}  sessions {}  rss {:.0} MB",
                    started.elapsed().as_secs_f64() * 1000.0,
                    status.load_per_cpu().unwrap_or(0.0),
                    status.cpu_count.unwrap_or(0),
                    status.uptime_seconds / 3600,
                    (status.uptime_seconds % 3600) / 60,
                    status.loaded_workspaces,
                    status.active_sessions,
                    status.memory_rss_mb().unwrap_or(0.0)
                );
                let engines: Vec<&str> = status
                    .detected_engines
                    .iter()
                    .filter(|e| e.as_str() != "generic-lsp")
                    .map(|e| e.split(' ').next().unwrap_or(e))
                    .collect();
                println!("{:<22} engines: {}", "", engines.join(", "));
            }
            Err(e) => println!("{node:<22} DOWN  {e}"),
        }
    }
    println!("────────────────────────────────────────────────────");
    println!("Workspace:           {workspace_name}");
    println!("Engine needed:       {}", engine.unwrap_or("(any)"));
    if let Some(home) = home {
        println!("Home node (hash):    {home}");
    }
    match remembered {
        Some(node) => println!("Placed on:           {node}"),
        None => println!("Placed on:           (not yet)"),
    }
    Ok(())
}

/// Typed remote verification: check / lint / test with parsed diagnostics.
async fn run_verify(
    remote: SocketAddr,
    kind: VerifyKind,
    filter: Option<String>,
    timeout_secs: u64,
    json: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let report = prod_code_mcp::verify::run_verify(
        remote,
        &root,
        Some(&cwd),
        kind,
        filter.as_deref(),
        timeout_secs,
    )
    .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render(200));
    }
    std::process::exit(if report.ok() { 0 } else { 1 });
}

/// Run a command remotely inside this checkout's server workspace copy and mirror its output.
async fn run_codemod_cli(
    remote: SocketAddr,
    rule: String,
    path: Option<String>,
    apply: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({ "rule": rule, "apply": apply });
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    let result = prod_code_mcp::tools::execute_tool(remote, &root, "code_codemod", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_search_cli(
    remote: SocketAddr,
    query: String,
    limit: usize,
    path: Option<String>,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let subpath = path.or_else(|| prod_code_mcp::exec::subdir_of(&root, &cwd));
    let resp =
        prod_code_mcp::search::search(remote, &root, &query, limit, subpath.as_deref()).await?;
    println!("{}", prod_code_mcp::search::render(&resp, &query));
    Ok(())
}

async fn run_slice(
    remote: SocketAddr,
    target: String,
    line: Option<u32>,
    character: u32,
    depth: u32,
    max_bytes: usize,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let (file, line, col) = match line {
        Some(line) => {
            let path = PathBuf::from(&target);
            let abs = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            (std::fs::canonicalize(&abs).unwrap_or(abs), line, character)
        }
        None => {
            let hit = prod_code_mcp::tools::resolve_symbol(remote, &root, &target, None).await?;
            println!(
                "{} {} at {}:{}:{}",
                hit.kind,
                hit.name,
                hit.path
                    .strip_prefix(&root)
                    .unwrap_or(&hit.path)
                    .to_string_lossy(),
                hit.line,
                hit.col
            );
            (hit.path, hit.line, hit.col)
        }
    };
    let report =
        prod_code_mcp::slice::slice(remote, &root, &file, line, col, depth, max_bytes).await?;
    println!("{}", report.render());
    Ok(())
}

async fn run_shadow_cli(
    remote: SocketAddr,
    spec: PathBuf,
    timeout_secs: u64,
    parallel: usize,
    apply: bool,
    command: Vec<String>,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let subdir = prod_code_mcp::exec::subdir_of(&root, &cwd);
    let text = std::fs::read_to_string(&spec)
        .with_context(|| format!("cannot read {}", spec.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("{} is not JSON", spec.display()))?;
    let specs = prod_code_mcp::shadow::parse_specs(&root, &json, spec.parent())?;
    let outcome = prod_code_mcp::shadow::run_shadow(
        remote,
        &root,
        subdir.as_deref(),
        &specs,
        command.clone(),
        vec![("CARGO_TERM_COLOR".to_string(), "never".to_string())],
        timeout_secs,
        parallel,
        16 * 1024,
    )
    .await?;
    let applied = match (apply, outcome.winner) {
        (true, Some(i)) => Some(prod_code_mcp::shadow::apply_hypothesis(&root, &specs[i])?),
        _ => None,
    };
    println!(
        "{}",
        prod_code_mcp::shadow::render_report(&outcome, &command, applied.as_deref(), 4000)
    );
    if outcome.winner.is_none() {
        std::process::exit(1);
    }
    Ok(())
}

async fn run_exec(
    remote: SocketAddr,
    command: Vec<String>,
    timeout_secs: u64,
    pull_changes: bool,
) -> Result<()> {
    use std::io::Write;
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    // Commands run where they were typed: a subdirectory of the checkout maps to the same
    // subdirectory of the server copy.
    let subdir = prod_code_mcp::exec::subdir_of(&root, &cwd);
    let mut env_pairs = Vec::new();
    if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        env_pairs.push(("CARGO_TERM_COLOR".to_string(), "always".to_string()));
    }
    let started = std::time::Instant::now();
    let outcome = prod_code_mcp::exec::run_remote(
        remote,
        &root,
        subdir.as_deref(),
        command.clone(),
        env_pairs,
        timeout_secs,
        pull_changes,
        |is_stderr, data| {
            if is_stderr {
                let mut e = std::io::stderr().lock();
                let _ = e.write_all(data);
                let _ = e.flush();
            } else {
                let mut o = std::io::stdout().lock();
                let _ = o.write_all(data);
                let _ = o.flush();
            }
        },
    )
    .await?;
    let exit = outcome.exit;
    if let Some(err) = &exit.error {
        anyhow::bail!("remote exec failed: {err}");
    }
    if !outcome.pulled_files.is_empty() {
        eprintln!(
            "[prod-code exec] {} file(s) changed by the command written back: {}",
            outcome.pulled_files.len(),
            outcome.pulled_files.join(", ")
        );
    }
    eprintln!(
        "[prod-code exec] {} in {:.1}s (server {:.1}s) on {}",
        match (exit.timed_out, exit.exit_code) {
            (true, _) => "timed out".to_string(),
            (false, Some(code)) => format!("exit {code}"),
            (false, None) => "killed".to_string(),
        },
        started.elapsed().as_secs_f64(),
        exit.duration_ms as f64 / 1000.0,
        exit.server_workspace_root
    );
    std::process::exit(exit.exit_code.unwrap_or(1));
}

/// Run concurrent pipelined benchmark against remote gateway across workspaces and worktrees.
async fn run_benchmark(
    remote: SocketAddr,
    workspaces: Vec<PathBuf>,
    concurrency: usize,
    depth: usize,
    duration_secs: u64,
) -> Result<()> {
    let target_workspaces: Vec<PathBuf> = if workspaces.is_empty() {
        vec![env::current_dir()?]
    } else {
        workspaces
    };

    println!("⚡ prod-code Multi-Tenant Benchmark (Pipelined Concurrent Load)");
    println!("────────────────────────────────────────────────────────────────");
    println!("Target Remote:       {remote}");
    println!("Concurrency:         {concurrency} worker connections");
    println!("Pipeline Depth:      {depth} in-flight queries per worker");
    println!("Duration:            {duration_secs}s");
    println!("Target Workspaces:   {} total", target_workspaces.len());
    for (i, ws) in target_workspaces.iter().enumerate() {
        let name = detect_workspace_name(ws).unwrap_or_else(|| "default".to_string());
        println!("  • [{}] {} (base: {})", i + 1, ws.display(), name);
    }
    println!("────────────────────────────────────────────────────────────────");
    println!("Connecting workers and starting load test...");

    let start_instant = std::time::Instant::now();
    let end_deadline = start_instant + std::time::Duration::from_secs(duration_secs);

    let mut handles = Vec::new();

    for worker_id in 0..concurrency {
        let ws_path = target_workspaces[worker_id % target_workspaces.len()].clone();
        let handle = tokio::spawn(async move {
            let mut latencies_us = Vec::new();
            let mut completed: u64 = 0;
            let mut errors: u64 = 0;

            let ws_str = ws_path.to_string_lossy().to_string();
            let base_name = detect_workspace_name(&ws_path);

            let stream = match TcpStream::connect(remote).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Worker {worker_id} connection failed: {e}");
                    return (completed, errors + 1, latencies_us);
                }
            };
            let _ = stream.set_nodelay(true);
            let mut framed = Framed::new(stream, ProdCodeCodec::new());

            // 1. Handshake
            let handshake = HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                client_name: format!("bench-worker-{worker_id}"),
                client_pid: std::process::id(),
                auth_token: None,
                client_workspace_root: ws_str.clone(),
                preferred_engine: None,
                base_workspace_name: base_name,
                engine_subpath: None,
                client_agent: Some(prod_code_protocol::detect_client_agent()),
                client_host: Some(prod_code_protocol::client_host()),
            };

            if framed
                .send(WireMessage::HandshakeRequest(handshake))
                .await
                .is_err()
            {
                return (completed, errors + 1, latencies_us);
            }

            match framed.next().await {
                Some(Ok(WireMessage::HandshakeResponse(_))) => {}
                _ => return (completed, errors + 1, latencies_us),
            }

            // 2. Initialize LSP
            let init_req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": format!("file://{ws_str}"),
                    "capabilities": {}
                }
            });

            if framed
                .send(WireMessage::LspPayload(init_req.to_string()))
                .await
                .is_err()
            {
                return (completed, errors + 1, latencies_us);
            }

            let _ = framed.next().await;

            let initialized = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            });
            let _ = framed
                .send(WireMessage::LspPayload(initialized.to_string()))
                .await;

            let (code_file, sym_line, sym_col) = find_first_code_file(&ws_path)
                .unwrap_or_else(|| (ws_path.join("src/main.rs"), 5, 5));
            let file_uri = format!("file://{}", code_file.to_string_lossy());

            let mut req_id: u64 = 2;
            let mut in_flight: std::collections::HashMap<u64, std::time::Instant> =
                std::collections::HashMap::new();

            while std::time::Instant::now() < end_deadline {
                // Keep the pipeline filled up to `depth`
                while in_flight.len() < depth && std::time::Instant::now() < end_deadline {
                    let id = req_id;
                    req_id += 1;
                    let hover_req = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "method": "textDocument/hover",
                        "params": {
                            "textDocument": { "uri": file_uri },
                            "position": { "line": sym_line, "character": sym_col }
                        }
                    });

                    let send_time = std::time::Instant::now();
                    if framed
                        .send(WireMessage::LspPayload(hover_req.to_string()))
                        .await
                        .is_ok()
                    {
                        in_flight.insert(id, send_time);
                    } else {
                        errors += 1;
                        break;
                    }
                }

                // Drain ready responses
                tokio::select! {
                    msg_opt = framed.next() => {
                        match msg_opt {
                            Some(Ok(WireMessage::LspPayload(payload))) => {
                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&payload) {
                                    let maybe_id = val.get("id").and_then(|v| v.as_u64());
                                    if let Some(send_time) = maybe_id.and_then(|id| in_flight.remove(&id)) {
                                        let elapsed = send_time.elapsed().as_micros() as u64;
                                        latencies_us.push(elapsed);
                                        completed += 1;
                                    }
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) | None => {
                                errors += 1;
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
                }
            }

            let _ = framed
                .send(WireMessage::Disconnect {
                    reason: "bench finished".to_string(),
                })
                .await;
            (completed, errors, latencies_us)
        });
        handles.push(handle);
    }

    let mut total_completed = 0;
    let mut total_errors = 0;
    let mut all_latencies = Vec::new();

    for h in handles {
        if let Ok((comp, errs, mut lats)) = h.await {
            total_completed += comp;
            total_errors += errs;
            all_latencies.append(&mut lats);
        }
    }

    let elapsed = start_instant.elapsed().as_secs_f64();
    let qps = if elapsed > 0.0 {
        (total_completed as f64) / elapsed
    } else {
        0.0
    };

    all_latencies.sort_unstable();
    let p50 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 50 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let p90 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 90 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let p95 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 95 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let p99 = if !all_latencies.is_empty() {
        all_latencies[all_latencies.len() * 99 / 100] as f64 / 1000.0
    } else {
        0.0
    };
    let min = if !all_latencies.is_empty() {
        all_latencies[0] as f64 / 1000.0
    } else {
        0.0
    };

    println!("\n📊 Benchmark Results:");
    println!("────────────────────────────────────────────────────────────────");
    println!("Elapsed Time:        {:.2}s", elapsed);
    println!("Completed Queries:   {}", total_completed);
    println!("Errors:              {}", total_errors);
    println!("Throughput:          \x1b[1;32m{:.1} QPS\x1b[0m", qps);
    println!("Latency (min):       {:.2} ms", min);
    println!("Latency (p50):       {:.2} ms", p50);
    println!("Latency (p90):       {:.2} ms", p90);
    println!("Latency (p95):       {:.2} ms", p95);
    println!("Latency (p99):       {:.2} ms", p99);
    println!("────────────────────────────────────────────────────────────────");

    Ok(())
}

/// Run the multi-worktree divergence and correctness benchmark against the remote gateway.
async fn run_divergent_bench(config: DivergentBenchConfig) -> Result<()> {
    println!("⚡ prod-code Divergent Worktree Benchmark (Multi-Agent Fleet Simulation)");
    println!("────────────────────────────────────────────────────────────────");
    println!("Target Remote:       {}", config.remote);
    println!(
        "Base Repo:           {}",
        config
            .base_repo
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<scratch fixture repo>".to_string())
    );
    println!("Workspace Mode:      {}", config.mode.label());
    println!(
        "Session Model:       {}",
        if config.persistent {
            "persistent"
        } else {
            "connect per query"
        }
    );
    println!("Concurrent Workers:  {}", config.workers);
    println!("Queries Per Worker:  {}", config.queries_per_worker);
    println!("────────────────────────────────────────────────────────────────");
    println!("Forking git worktrees, applying controlled mutations, syncing to gateway...");

    let report = divergent_bench::run(config).await?;
    report.print();

    if !report.all_passed {
        anyhow::bail!("divergent worktree benchmark FAILED correctness verification");
    }

    Ok(())
}
