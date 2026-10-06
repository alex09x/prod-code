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

/// Whether an error is an initial transport connection failure that can be safely retried.
///
/// Mutating tools (`code_exec`) must NEVER be automatically retried: if the gateway received
/// the command and dropped connection during execution, replaying it on another node can
/// cause duplicate external side-effects (e.g. database migrations, external API calls).
/// Similarly, any error where the command result is unknown is non-retryable.
pub fn is_retryable_connection_error(tool_name: &str, e: &anyhow::Error) -> bool {
    if tool_name == "code_exec" {
        return false;
    }
    let msg = format!("{e:#}").to_lowercase();
    if msg.contains("result is unknown")
        || msg.contains("during exec")
        || msg.contains("command's result is unknown")
    {
        return false;
    }
    msg.contains("failed to connect to remote gateway")
        || msg.contains("connection refused")
        || msg.contains("no route to host")
        || msg.contains("network is unreachable")
        || msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("broken pipe")
        || msg.contains("connection reset")
        || msg.contains("connection closed")
        || msg.contains("transport closed")
        || msg.contains("server closed connection")
        || msg.contains("capacity: this node has no memory")
        || msg.contains("gateway refused the session: capacity")
        || msg.contains("this node has no memory for a new")
        || msg.contains("refused for capacity")
        || msg.contains("capacity admission")
        || msg.contains("no space left on device")
        || msg.contains("short of disk")
}

/// Run a 250ms UDP discovery probe (multicast + unicast to the old address) and return
/// the best live node that can serve this workspace's engine and OS requirements.
///
/// Routing priority:
/// 1. Only consider nodes that support the required engine and OS (e.g., Swift requires macOS)
/// 2. Nodes that already have the current workspace loaded (warm engine, no cold start)
/// 3. Among those (or all candidates if none has it), pick the one with the most available memory
///    and the lowest load.
pub async fn rediscover_node(old: SocketAddr, workspace_root: &Path) -> Option<SocketAddr> {
    let seeds = vec![old];
    let nodes =
        tokio::task::spawn_blocking(move || prod_code_protocol::discovery::discover(&seeds))
            .await
            .ok()?;

    if nodes.is_empty() {
        return None;
    }

    // Detect required engine and OS for this workspace.
    let (subproject, detected_engine) = crate::sync::engine_project(workspace_root, workspace_root);
    let macos_cgo = match detected_engine {
        Some("go") => {
            let target_dir = subproject
                .as_ref()
                .map(|sub| workspace_root.join(sub))
                .unwrap_or_else(|| workspace_root.to_path_buf());
            crate::sync::macos_only_cgo(&target_dir)
        }
        _ => None,
    };
    let needs_macos = detected_engine == Some("swift") || macos_cgo.is_some();

    // Log what we found.
    for n in &nodes {
        tracing::debug!(
            addr = %n.addr,
            engines = ?n.engines,
            cpus = n.cpus,
            mem_avail_mb = n.mem_avail_mb,
            mem_total_mb = n.mem_total_mb,
            rss_mb = n.rss_mb,
            load = n.load_per_cpu,
            sessions = n.sessions,
            workspaces = n.workspaces.len(),
            "discovered node"
        );
    }

    // Exclude the dead node (unless it's the only one that answered).
    let viable: Vec<_> = nodes.iter().filter(|n| n.addr != old).collect();
    let viable = if viable.is_empty() {
        nodes.iter().collect()
    } else {
        viable
    };

    // Filter by required engine and OS!
    let candidates: Vec<_> = viable
        .into_iter()
        .filter(|n| {
            if let Some(engine) = detected_engine {
                let has_engine = n.engines.iter().any(|e| {
                    e.as_str() == engine
                        || e.strip_prefix(engine)
                            .is_some_and(|rest| rest.starts_with(' '))
                });
                if !has_engine {
                    return false;
                }
            }
            if needs_macos {
                let has_macos = n
                    .engines
                    .iter()
                    .any(|e| e == "swift" || e.starts_with("swift "));
                if !has_macos {
                    return false;
                }
            }
            true
        })
        .collect();

    if candidates.is_empty() {
        tracing::warn!(
            engine = ?detected_engine,
            needs_macos,
            "no discovered nodes support the required engine/OS for this workspace"
        );
        return None;
    }

    // The workspace identity (directory basename) — matches what the gateway uses.
    let ws_name = workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    // Prefer a node that already has this workspace loaded (warm engine),
    // provided it has sufficient memory (at least 512 MB available).
    let warm: Vec<_> = candidates
        .iter()
        .filter(|n| n.workspaces.iter().any(|w| w.name == ws_name) && n.mem_avail_mb >= 512)
        .copied()
        .collect();

    // Prefer nodes with at least 512 MB available memory if any exist.
    let roomy: Vec<_> = candidates
        .iter()
        .filter(|n| n.mem_avail_mb >= 512)
        .copied()
        .collect();

    let pool = if !warm.is_empty() {
        &warm
    } else if !roomy.is_empty() {
        &roomy
    } else {
        &candidates
    };

    // Score: more available memory is better, lower load is better.
    pool.iter()
        .max_by(|a, b| {
            let score_a = a.mem_avail_mb as f64 - a.load_per_cpu * 10000.0;
            let score_b = b.mem_avail_mb as f64 - b.load_per_cpu * 10000.0;
            score_a
                .partial_cmp(&score_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|n| n.addr)
}
