//! The gateway daemon: parse the command line, set logging up, and run the server.

use anyhow::Result;
use clap::Parser;
use prod_code_gateway::ServerCli;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "info,prod_code_gateway=debug,prod_code_engine_rust=debug".into()
            }),
        )
        .init();
    prod_code_gateway::run(ServerCli::parse()).await
}
