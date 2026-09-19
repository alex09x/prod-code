//! prod-code client: ultra-thin CLI bridge for editors and AI coding agents over 10G LAN.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
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
    /// Remote gateway address (host:port). Defaults to PROD_CODE_REMOTE env var or 127.0.0.1:9400.
    #[arg(
        short,
        long,
        env = "PROD_CODE_REMOTE",
        default_value = "127.0.0.1:9400"
    )]
    remote: SocketAddr,

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
    /// List document outline symbols: prod-code symbols <file>
    Symbols { file: PathBuf },
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Commands::Lsp) {
        Commands::Lsp => run_lsp_bridge(cli.remote).await,
        Commands::Status => run_status_probe(cli.remote).await,
        Commands::Mcp => run_mcp_server(cli.remote).await,
        Commands::Sync { path } => run_sync(cli.remote, path).await,
        Commands::Def { file, line, col } => run_definition(cli.remote, &file, line, col).await,
        Commands::Hover { file, line, col } => run_hover(cli.remote, &file, line, col).await,
        Commands::Refs { file, line, col } => run_references(cli.remote, &file, line, col).await,
        Commands::Symbols { file } => run_symbols(cli.remote, &file).await,
        Commands::Bench {
            workspaces,
            concurrency,
            depth,
            duration_secs,
        } => run_benchmark(cli.remote, workspaces, concurrency, depth, duration_secs).await,
    }
}

/// Dynamically detect the base repository name if current directory is a git worktree or repository.
pub fn detect_workspace_name(dir: &Path) -> Option<String> {
    let dot_git = dir.join(".git");
    if dot_git.is_file() {
        if let Ok(content) = std::fs::read_to_string(&dot_git) {
            for line in content.lines() {
                if let Some(gitdir) = line.trim().strip_prefix("gitdir:") {
                    let gitdir_path = PathBuf::from(gitdir.trim());
                    let mut cur = gitdir_path.as_path();
                    while let Some(parent) = cur.parent() {
                        if cur.file_name().is_some_and(|n| n == ".git") {
                            return parent
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map(|s| s.to_string());
                        }
                        cur = parent;
                    }
                }
            }
        }
    } else if dot_git.is_dir() {
        return dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string());
    }

    dir.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
}

/// Helper to connect, initialize, and execute a targeted LSP request against the remote gateway.
async fn execute_lsp_query(
    remote: SocketAddr,
    file_path: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let cwd = env::current_dir().context("Failed to determine current working directory")?;
    let cwd_str = cwd.to_string_lossy().to_string();

    let abs_path = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        cwd.join(file_path)
    };

    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", abs_path))?
        .to_string();

    let file_content = tokio::fs::read_to_string(&abs_path)
        .await
        .with_context(|| format!("Failed to read file {:?}", abs_path))?;

    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    // 1. Handshake
    framed
        .send(WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: PROTOCOL_VERSION,
            client_name: "prod-code-cli".to_string(),
            client_pid: std::process::id(),
            auth_token: None,
            client_workspace_root: cwd_str,
            preferred_engine: None,
            base_workspace_name: detect_workspace_name(&cwd),
        }))
        .await?;

    let _ = match framed.next().await {
        Some(Ok(WireMessage::HandshakeResponse(resp))) => resp,
        other => anyhow::bail!("Unexpected handshake response: {:?}", other),
    };

    // 2. LSP Initialize
    let folder_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": format!("file://{}", cwd.to_string_lossy()),
            "workspaceFolders": [
                {
                    "name": folder_name,
                    "uri": format!("file://{}", cwd.to_string_lossy())
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
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    framed
        .send(WireMessage::LspPayload(initialized.to_string()))
        .await?;

    // 4. LSP didOpen notification
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": file_uri,
                "languageId": "rust",
                "version": 1,
                "text": file_content
            }
        }
    });
    framed
        .send(WireMessage::LspPayload(did_open.to_string()))
        .await?;

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
                5 => "Class/Struct",
                6 => "Method",
                11 => "Function",
                12 => "Variable",
                13 => "Constant",
                23 => "Struct",
                _ => "Symbol",
            };
            let start_line = sym
                .get("range")
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
    let deltas = prod_code_mcp::scan_workspace_files(&cwd, subpath.as_deref())?;
    let file_count = deltas.len();

    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    let req = prod_code_protocol::SyncRequest {
        client_workspace_root: cwd.to_string_lossy().to_string(),
        files: deltas,
        clean_others: false,
        base_workspace_name: detect_workspace_name(&cwd),
    };

    framed.send(WireMessage::SyncRequest(req)).await?;

    if let Some(msg_res) = framed.next().await {
        match msg_res? {
            WireMessage::SyncResponse(resp) => {
                let total_ms = start.elapsed().as_millis();
                let kb = (resp.bytes_transferred as f64) / 1024.0;
                println!(
                    "⚡ prod-code Fast-Sync Completed in {}ms (server: {}ms)",
                    total_ms, resp.duration_ms
                );
                println!("────────────────────────────────────────────────────");
                println!("Local Workspace:   {}", cwd.display());
                println!("Remote Workspace:  {}", resp.server_workspace_root);
                println!("Files Scanned:     {}", file_count);
                println!("Files Updated:     {}", resp.files_updated);
                println!("Files Deleted:     {}", resp.files_deleted);
                println!("Data Transferred:  {:.1} KB", kb);
                println!("Status:            SYNCHRONIZED");
            }
            other => anyhow::bail!("Unexpected response from gateway: {:?}", other),
        }
    } else {
        anyhow::bail!("Remote gateway closed connection prematurely without sync response");
    }

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
                return Some((
                    path.to_path_buf(),
                    line_idx as u32,
                    (pos + 12) as u32,
                ));
            }
            if let Some(pos) = trimmed.find("pub struct ") {
                return Some((
                    path.to_path_buf(),
                    line_idx as u32,
                    (pos + 13) as u32,
                ));
            }
            if let Some(pos) = trimmed.find("pub fn ") {
                return Some((
                    path.to_path_buf(),
                    line_idx as u32,
                    (pos + 9) as u32,
                ));
            }
            if let Some(pos) = trimmed.find("func ") {
                return Some((
                    path.to_path_buf(),
                    line_idx as u32,
                    (pos + 6) as u32,
                ));
            }
        }
    }
    None
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
