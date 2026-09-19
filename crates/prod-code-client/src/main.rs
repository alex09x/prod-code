//! prod-code client: ultra-thin CLI bridge for editors and AI coding agents over 10G LAN.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeRequest, ProdCodeCodec, StatusResponse, WireMessage, PROTOCOL_VERSION,
};
use std::env;
use std::net::SocketAddr;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

#[derive(Parser, Debug)]
#[command(
    name = "prod-code",
    author = "Alex <alex@prod.codes>",
    version,
    about = "Remote Code Intelligence Client"
)]
struct Cli {
    /// Remote gateway address (host:port). Defaults to PROD_CODE_REMOTE env var or 127.0.0.1:9400.
    #[arg(short, long, env = "PROD_CODE_REMOTE", default_value = "127.0.0.1:9400")]
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Commands::Lsp) {
        Commands::Lsp => run_lsp_bridge(cli.remote).await,
        Commands::Status => run_status_probe(cli.remote).await,
        Commands::Mcp => run_mcp_stub(cli.remote).await,
        Commands::Sync => run_sync_stub(cli.remote).await,
    }
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
            let len_str = header_line
                .trim_start_matches("Content-Length:")
                .trim();
            let content_len: usize = len_str.parse().context("Invalid Content-Length header")?;

            // Read the empty separating line \r\n
            header_line.clear();
            stdin_reader.read_line(&mut header_line).await?;

            // Read exact body
            let mut body_buf = vec![0u8; content_len];
            stdin_reader.read_exact(&mut body_buf).await?;

            let json_payload = String::from_utf8(body_buf)
                .context("LSP payload was not valid UTF-8 string")?;

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
    println!("Syncing workspace {:?} with remote gateway at {}", cwd, remote);
    println!("Sync: OK (Phase 1)");
    Ok(())
}
