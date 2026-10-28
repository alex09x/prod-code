/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::future::join_all;
use std::net::SocketAddr;

pub(super) async fn rebalance_target_satisfies_os(
    target: SocketAddr,
    required_os: Option<&str>,
) -> bool {
    let Some(required_os) = required_os else {
        return true;
    };
    prod_code_mcp::cluster::node_status(target)
        .await
        .is_ok_and(|status| prod_code_mcp::cluster::runs_os(&status, required_os))
}

pub(super) async fn ask_rebalance_cluster(
    nodes: &[SocketAddr],
    workspace_name: &str,
    engine: Option<&str>,
    required_os: Option<&str>,
) -> Option<(SocketAddr, String, usize)> {
    let view_seeds = prod_code_mcp::cluster::rendezvous_order(nodes, workspace_name);
    let mut holders = Vec::new();
    let mut fallback_seed = None;
    let mut views_complete = true;
    let base_name = workspace_name.split('#').next().unwrap_or(workspace_name);

    // A successful seed may not hold this workspace; collect each configured node's local
    // workspace view before deciding who needs the redirect request.
    let views = join_all(
        view_seeds
            .iter()
            .copied()
            .map(prod_code_mcp::cluster::cluster_view),
    )
    .await;
    for (seed, view) in view_seeds.into_iter().zip(views) {
        let Ok(view) = view else {
            views_complete = false;
            continue;
        };
        fallback_seed.get_or_insert(seed);
        // Placement only redirects sessions held by that gateway's local manager.
        for peer in view.nodes {
            if !peer.alive
                || !peer.workspaces.iter().any(|workspace| {
                    workspace.name == workspace_name || workspace.name == base_name
                })
            {
                continue;
            }
            let holder = peer.addr.parse::<SocketAddr>().ok()?;
            if !nodes.contains(&holder) {
                return None;
            }
            if !holders.contains(&holder) {
                holders.push(holder);
            }
        }
    }
    if !views_complete {
        // Without every configured node's view, this client cannot know that it found
        // every holder, so it must not trigger or report a partial active rebalance.
        return None;
    }

    if holders.is_empty() {
        return ask_rebalance_target(fallback_seed?, workspace_name, engine, required_os).await;
    }

    let mut selected = None;
    // Preview every holder before any redirect so conflicting gossip cannot move sessions
    // to different targets.
    let previews = join_all(holders.iter().map(|holder| {
        prod_code_mcp::cluster::ask_placement_preview(*holder, workspace_name, engine, required_os)
    }))
    .await;
    for preview in previews {
        let preview = preview.ok()?;
        let target = preview
            .node
            .as_deref()
            .and_then(|addr| addr.parse::<SocketAddr>().ok())?;
        if !rebalance_target_satisfies_os(target, required_os).await {
            return None;
        }
        if let Some((selected_target, _)) = selected {
            if selected_target != target {
                return None;
            }
        } else {
            selected = Some((target, preview.reason));
        }
    }

    let (target, reason) = selected?;
    let mut notified_sessions = 0usize;
    let responses = join_all(holders.iter().map(|holder| {
        prod_code_mcp::cluster::ask_placement_opt(
            *holder,
            workspace_name,
            engine,
            required_os,
            true,
        )
    }))
    .await;
    for response in responses {
        let response = response.ok()?;
        let actual_target = response
            .node
            .as_deref()
            .and_then(|addr| addr.parse::<SocketAddr>().ok())?;
        if actual_target != target {
            return None;
        }
        if !rebalance_target_satisfies_os(actual_target, required_os).await {
            return None;
        }
        notified_sessions = notified_sessions.saturating_add(response.notified_sessions);
    }

    Some((target, reason, notified_sessions))
}

pub(super) async fn ask_rebalance_target(
    seed: SocketAddr,
    workspace_name: &str,
    engine: Option<&str>,
    required_os: Option<&str>,
) -> Option<(SocketAddr, String, usize)> {
    if required_os.is_some() {
        // Only gateways that understand this side-effect-free preview can safely rebalance
        // OS-constrained active sessions during a rolling upgrade.
        let preview = prod_code_mcp::cluster::ask_placement_preview(
            seed,
            workspace_name,
            engine,
            required_os,
        )
        .await
        .ok()?;
        let preview_target = preview
            .node
            .as_deref()
            .and_then(|addr| addr.parse::<SocketAddr>().ok())?;
        if !rebalance_target_satisfies_os(preview_target, required_os).await {
            return None;
        }
    }

    let response =
        prod_code_mcp::cluster::ask_placement_opt(seed, workspace_name, engine, required_os, true)
            .await
            .ok()?;
    let target = response
        .node
        .as_deref()
        .and_then(|addr| addr.parse::<SocketAddr>().ok())?;
    if !rebalance_target_satisfies_os(target, required_os).await {
        return None;
    }
    Some((target, response.reason, response.notified_sessions))
}
