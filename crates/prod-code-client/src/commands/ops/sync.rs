/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use futures_util::SinkExt;
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio_util::codec::Framed;

pub async fn run_mcp_server(remote: SocketAddr) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    prod_code_mcp::run_stdio_mcp_server(remote, cwd).await
}

pub async fn run_sync(remote: SocketAddr, subpath: Option<PathBuf>) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let start = std::time::Instant::now();
    let identity = prod_code_mcp::sync::workspace_identity(&cwd);

    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
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

pub async fn run_pull(remote: SocketAddr, root: &Path, files: Vec<PathBuf>) -> Result<()> {
    let touched = prod_code_mcp::sync::pull_remote_files(remote, root, &files).await?;
    if !touched.is_empty() {
        println!(
            "📥 Successfully pulled {} file(s) from gateway:\n  {}",
            touched.len(),
            touched.join("\n  ")
        );
    } else {
        println!("No files were pulled.");
    }
    Ok(())
}
