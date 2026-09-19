//! prod-code client: ultra-thin CLI bridge for editors and AI coding agents over 10G LAN.

use clap::{Parser, Subcommand};
use std::net::SocketAddr;

#[derive(Parser, Debug)]
#[command(name = "prod-code", author, version, about = "Remote Code Intelligence Client")]
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
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command.unwrap_or(Commands::Lsp) {
        Commands::Lsp => {
            tracing::info!("Starting prod-code LSP bridge connecting to {}", cli.remote);
            // Stdio <-> TCP bridge implementation
            println!("prod-code LSP bridge ready (target: {})", cli.remote);
        }
        Commands::Mcp => {
            tracing::info!("Starting prod-code MCP server connecting to {}", cli.remote);
            println!("prod-code MCP server ready (target: {})", cli.remote);
        }
        Commands::Status => {
            println!("Target remote gateway: {}", cli.remote);
            println!("Status: Standby (Phase 1)");
        }
        Commands::Sync => {
            println!("Syncing worktree to remote gateway: {}", cli.remote);
        }
    }

    Ok(())
}
