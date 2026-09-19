//! prod-code gateway daemon: multi-tenant server for remote code intelligence over 10 GbE LAN.

use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::TcpListener;

#[derive(Parser, Debug)]
#[command(name = "prod-code-server", author, version, about = "Remote Code Intelligence Gateway")]
struct ServerCli {
    /// Bind address (IP:port). Defaults to 0.0.0.0:9400.
    #[arg(short, long, env = "PROD_CODE_BIND", default_value = "0.0.0.0:9400")]
    bind: SocketAddr,

    /// Workspace root storage directory on server.
    #[arg(short, long, env = "PROD_CODE_STORAGE", default_value = "/srv/prod-code/workspaces")]
    storage: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = ServerCli::parse();

    tracing::info!(
        "prod-code gateway daemon starting on {} (storage: {:?})",
        cli.bind,
        cli.storage
    );

    let listener = TcpListener::bind(cli.bind).await?;
    tracing::info!("prod-code gateway listening for 10G LAN connections on {}", cli.bind);

    loop {
        let (socket, addr) = listener.accept().await?;
        tracing::info!("Accepted connection from client {}", addr);
        tokio::spawn(async move {
            tracing::debug!("Handling session for {}", addr);
            // Session loop will be wired to language engine dispatch
            drop(socket);
        });
    }
}
