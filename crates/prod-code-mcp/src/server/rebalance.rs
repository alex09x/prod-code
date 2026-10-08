/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

/// Evaluates cluster rebalancing for the current workspace and pre-warms the target node
/// if a more efficient node is recommended. Rejects filesystem roots and unbounded non-git directories.
pub(crate) async fn handle_cluster_rebalance_tick(remote: &mut SocketAddr, workspace_root: &Path) {
    let canonical_root =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    if crate::sync::is_filesystem_root(&canonical_root) {
        return;
    }
    let has_git = crate::sync::git::git_head(&canonical_root).is_ok();
    let (_, engine) = crate::sync::engine_project(workspace_root, workspace_root);
    if !has_git && engine.is_none() {
        return;
    }

    let identity = crate::sync::workspace_identity(workspace_root);
    let ws_name = identity.base.as_ref().unwrap_or(&identity.name).clone();
    let os = crate::sync::macos_only_cgo(workspace_root).map(|_| "macos");
    if let Some((new_addr, reason)) =
        crate::cluster::evaluate_cluster_rebalance(*remote, &ws_name, engine, os).await
    {
        tracing::info!(
            old = %remote,
            new = %new_addr,
            %reason,
            "cluster rebalance: migrating active workspace to more efficient node"
        );
        *remote = new_addr;
        crate::cluster::remember_placement(&ws_name, new_addr);
        // Pre-warm the workspace on the new node in the background
        let root_clone = workspace_root.to_path_buf();
        let identity_clone = identity.clone();
        tokio::spawn(async move {
            if let Ok(stream) = prod_code_protocol::transport::connect(new_addr).await {
                let mut framed = tokio_util::codec::Framed::new(
                    stream,
                    prod_code_protocol::ProdCodeCodec::new(),
                );
                let _ = crate::sync::push_workspace_sync(
                    &mut framed,
                    &root_clone,
                    &identity_clone,
                    None,
                )
                .await;
            }
        });
    }
}
