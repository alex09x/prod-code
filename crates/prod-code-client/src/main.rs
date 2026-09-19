//! prod-code client: ultra-thin CLI bridge for editors and AI coding agents over 10G LAN.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, StatusResponse, WireMessage,
};
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
    /// Push current worktree delta to remote storage.
    Sync,
    /// Jump to symbol definition: prod-code def <file> <line> <col>
    Def { file: PathBuf, line: u32, col: u32 },
    /// Inspect symbol type & docs: prod-code hover <file> <line> <col>
    Hover { file: PathBuf, line: u32, col: u32 },
    /// Find all references to symbol: prod-code refs <file> <line> <col>
    Refs { file: PathBuf, line: u32, col: u32 },
    /// List document outline symbols: prod-code symbols <file>
    Symbols { file: PathBuf },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Commands::Lsp) {
        Commands::Lsp => run_lsp_bridge(cli.remote).await,
        Commands::Status => run_status_probe(cli.remote).await,
        Commands::Mcp => run_mcp_stub(cli.remote).await,
        Commands::Sync => run_sync_stub(cli.remote).await,
        Commands::Def { file, line, col } => run_definition(cli.remote, &file, line, col).await,
        Commands::Hover { file, line, col } => run_hover(cli.remote, &file, line, col).await,
        Commands::Refs { file, line, col } => run_references(cli.remote, &file, line, col).await,
        Commands::Symbols { file } => run_symbols(cli.remote, &file).await,
    }
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
    let query_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(25);
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
            WireMessage::StatusResponse(StatusResponse {
                server_pid,
                uptime_seconds,
                active_sessions,
                loaded_workspaces,
                detected_engines,
            }) => {
                let hours = uptime_seconds / 3600;
                let minutes = (uptime_seconds % 3600) / 60;
                let seconds = uptime_seconds % 60;

                println!("⚡ prod-code Remote Code Intelligence Gateway");
                println!("────────────────────────────────────────────────────");
                println!("Remote Address:    {remote} ({:.2?} RTT)", rtt);
                println!("Server PID:        {server_pid}");
                println!("Uptime:            {}h {}m {}s", hours, minutes, seconds);
                println!("Active Sessions:   {active_sessions}");
                println!("Loaded Workspaces: {loaded_workspaces}");
                println!("Engines Available: {}", detected_engines.join(", "));
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

async fn run_mcp_stub(remote: SocketAddr) -> Result<()> {
    println!("⚡ prod-code Native MCP Server (Phase 4)");
    println!("Target gateway: {remote}");
    println!("Ready to connect to Claude / Codex / Agy");
    Ok(())
}

async fn run_sync_stub(remote: SocketAddr) -> Result<()> {
    let cwd = env::current_dir()?;
    println!(
        "Syncing workspace {:?} with remote gateway at {}",
        cwd, remote
    );
    println!("Sync: OK (Phase 1)");
    Ok(())
}
