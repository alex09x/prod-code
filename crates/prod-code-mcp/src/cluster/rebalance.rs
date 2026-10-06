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

use super::discover::{cluster_view, node_status};
use super::parse::resolve_auto_remotes;
use super::routing::ROUTING;
use super::selection::{choose_best_node, runs_os, status_fits};

/// Evaluates whether the current remote node has degraded or become congested, and whether
/// a significantly more efficient, roomier node is available in the cluster.
/// Returns Some((new_addr, reason)) if rebalancing is recommended; None to remain on current node.
pub async fn evaluate_cluster_rebalance(
    current_node: SocketAddr,
    workspace_name: &str,
    engine: Option<&str>,
    os: Option<&str>,
) -> Option<(SocketAddr, String)> {
    if std::env::var_os("PROD_CODE_NO_REBALANCE").is_some() {
        return None;
    }

    // Single node / pinned check: if ROUTING is set and only lists current_node, do not rebalance.
    let cluster_nodes = if let Some(routing) = ROUTING.get() {
        if routing.nodes.len() <= 1 {
            return None;
        }
        routing.nodes.clone()
    } else {
        match resolve_auto_remotes() {
            Ok(nodes) if nodes.len() > 1 => nodes,
            _ => vec![current_node],
        }
    };

    evaluate_cluster_rebalance_with(&cluster_nodes, current_node, workspace_name, engine, os).await
}

/// [`evaluate_cluster_rebalance`] over an explicit list of cluster nodes.
pub async fn evaluate_cluster_rebalance_with(
    cluster_nodes: &[SocketAddr],
    current_node: SocketAddr,
    _workspace_name: &str,
    engine: Option<&str>,
    os: Option<&str>,
) -> Option<(SocketAddr, String)> {
    if std::env::var_os("PROD_CODE_NO_REBALANCE").is_some() || cluster_nodes.len() <= 1 {
        return None;
    }

    // Check status of current node
    let current_status = match node_status(current_node).await {
        Ok(s) => s,
        Err(_) => {
            // Current node is down: pick the best available node immediately, keeping
            // generic (non-macOS) work off macOS nodes unless no other peer exists.
            let mut candidates = Vec::new();
            let mut macos_fallback = Vec::new();
            for node in cluster_nodes {
                if *node == current_node {
                    continue;
                }
                if let Ok(st) = node_status(*node).await
                    && status_fits(&st, engine, os)
                    && st.host.pressure().is_none()
                {
                    if os.is_none() && runs_os(&st, "macos") {
                        macos_fallback.push((*node, st.congestion_score()));
                    } else {
                        candidates.push((*node, st.congestion_score()));
                    }
                }
            }
            let chosen =
                choose_best_node(&candidates).or_else(|| choose_best_node(&macos_fallback));
            if let Some(best) = chosen {
                return Some((
                    best,
                    format!(
                        "current node {current_node} is unreachable, migrating to live peer {best}"
                    ),
                ));
            }
            return None;
        }
    };

    let current_score = current_status.congestion_score();
    let has_pressure = current_status.host.pressure().is_some();

    // If current node is healthy and not congested (< 0.80 score and no pressure), keep warm caches
    if !has_pressure && current_score < 0.80 {
        return None;
    }

    // Try to get whole cluster state via cluster_view from current node or live peers
    let mut candidate_scores: Vec<(SocketAddr, f64, String)> = Vec::new();

    if let Ok(view) = cluster_view(current_node).await {
        for peer in view.nodes {
            if !peer.alive {
                continue;
            }
            let Ok(addr) = peer.addr.parse::<SocketAddr>() else {
                continue;
            };
            if addr == current_node {
                continue;
            }
            if !status_fits(&peer.status, engine, os) {
                continue;
            }
            if os.is_none() && runs_os(&peer.status, "macos") {
                // Don't migrate non-macOS work to macOS unless required
                continue;
            }
            let score = peer.status.congestion_score();
            candidate_scores.push((addr, score, peer.status.host.describe()));
        }
    } else {
        // Fallback: probe known nodes directly
        for node in cluster_nodes {
            if *node == current_node {
                continue;
            }
            if let Ok(st) = node_status(*node).await
                && status_fits(&st, engine, os)
            {
                if os.is_none() && runs_os(&st, "macos") {
                    continue;
                }
                candidate_scores.push((*node, st.congestion_score(), st.host.describe()));
            }
        }
    }

    if candidate_scores.is_empty() {
        return None;
    }

    // Filter out candidates under hard pressure (>= 1000.0) if any non-pressured candidate exists
    let non_pressured: Vec<_> = candidate_scores
        .iter()
        .filter(|(_, s, _)| *s < 1000.0)
        .cloned()
        .collect();
    let pool = if !non_pressured.is_empty() {
        &non_pressured
    } else {
        &candidate_scores
    };

    // Find best candidate
    let best = pool
        .iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;

    let (best_addr, best_score, host_desc) = (best.0, best.1, &best.2);

    // Migration criteria with hysteresis:
    // 1. Current node is under hard pressure (score >= 1000.0) and best node is not
    if has_pressure && best_score < 1000.0 {
        return Some((
            best_addr,
            format!(
                "node {current_node} is under resource pressure ({}), migrating to roomier {best_addr} (score {:.2}, {host_desc})",
                current_status.host.pressure().unwrap_or_default(),
                best_score
            ),
        ));
    }

    // 2. Current node is noticeably congested (score >= 0.80), best node is at least 2x better and diff >= 0.40
    if current_score >= 0.80
        && best_score < current_score * 0.50
        && (current_score - best_score) >= 0.40
    {
        return Some((
            best_addr,
            format!(
                "rebalancing from congested node {current_node} (score {:.2}) to quieter {best_addr} (score {:.2}, {host_desc})",
                current_score, best_score
            ),
        ));
    }

    None
}
