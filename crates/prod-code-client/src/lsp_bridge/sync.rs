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
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::codec::Framed;

/// How often the editor bridge looks for local changes to push to the node (#316).
pub const BRIDGE_SYNC_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// How long the editor bridge waits after a failed push before it tries again.
pub const BRIDGE_SYNC_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

/// Pushes the checkout's changes since the last sync on a connection of its own.
pub async fn push_checkout(remote: SocketAddr, root: &Path) -> Result<()> {
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let identity = prod_code_mcp::sync::workspace_identity(root);
    prod_code_mcp::sync::push_workspace_sync(&mut framed, root, &identity, None).await?;
    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "sync finished".to_string(),
        })
        .await;
    Ok(())
}

/// Keeps the node's copy of `root` current while an editor runs `prod-code lsp`: whenever the
/// file watcher saw a change (a save, a checkout, a generated file), the delta is pushed on a
/// connection of its own, so the language server session never waits for it (#316). `pushing`
/// is held for each push, which a save also takes.
pub async fn keep_checkout_synced(remote: SocketAddr, root: PathBuf, pushing: Arc<Mutex<()>>) {
    loop {
        tokio::time::sleep(BRIDGE_SYNC_POLL).await;
        let generation = prod_code_mcp::watch::current_generation(&root);
        if !prod_code_mcp::watch::sync_due(&root, generation) {
            continue;
        }
        let pushed = {
            let _one_at_a_time = pushing.lock().await;
            push_checkout(remote, &root).await
        };
        match pushed {
            Ok(()) => prod_code_mcp::watch::mark_synced(&root, generation),
            Err(err) => {
                tracing::debug!(error = %format!("{err:#}"), "background sync failed");
                tokio::time::sleep(BRIDGE_SYNC_RETRY).await;
            }
        }
    }
}
