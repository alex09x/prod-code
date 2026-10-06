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

use anyhow::{Result, anyhow};

use super::discover::{ask_placement, node_status};
use super::placement::{load_placement, placement_path, save_placement};
use super::rebalance::evaluate_cluster_rebalance_with;
use super::selection::{
    RESTART_RETRIES, RESTART_WAIT, choose_best_node, is_alive, node_fits, os_name,
    rendezvous_order, runs_os, status_fits,
};

/// Chooses the gateway for `workspace_name` among `nodes`: the remembered placement when it
/// is still one of the nodes, alive and able to serve `engine`, otherwise the quietest alive
/// node that can serve `engine`, in rendezvous order, which is then remembered. A single
/// node is returned as is, unless it must run `os` and does not. `engine` is the engine the
/// checkout needs (`swift` only runs on a macOS node, for example); `os` is the OS it needs (a
/// Go module whose cgo includes macOS headers, #248); `None` accepts any node.
pub async fn pick_node(
    nodes: &[SocketAddr],
    workspace_name: &str,
    engine: Option<&str>,
    os: Option<&str>,
) -> Result<SocketAddr> {
    pick_node_with(
        nodes,
        workspace_name,
        engine,
        os,
        placement_path().as_deref(),
    )
    .await
}

pub async fn pick_node_with(
    nodes: &[SocketAddr],
    workspace_name: &str,
    engine: Option<&str>,
    os: Option<&str>,
    placement_file: Option<&Path>,
) -> Result<SocketAddr> {
    let listed = |nodes: &[SocketAddr]| {
        nodes
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let [only] = nodes else {
        if nodes.is_empty() {
            return Err(anyhow!("no gateway addresses given"));
        }
        let mut placement = placement_file.map(load_placement).unwrap_or_default();
        if let Some(remembered) = placement.workspaces.get(workspace_name).copied() {
            if nodes.contains(&remembered) {
                let mut still_fits = node_fits(remembered, engine, os).await;
                // A node that does not answer at all may be restarting: ask again before moving.
                // One that answers but cannot serve the engine is left at once.
                let mut tries = 0;
                while !still_fits && tries < RESTART_RETRIES && !is_alive(remembered).await {
                    tokio::time::sleep(RESTART_WAIT).await;
                    tries += 1;
                    still_fits = node_fits(remembered, engine, os).await;
                }
                if still_fits && let Ok(status) = node_status(remembered).await {
                    if status.host.pressure().is_some() {
                        still_fits = false;
                    } else if status.congestion_score() >= 0.80 && nodes.len() > 1 {
                        // If remembered node has become congested, evaluate whether a significantly
                        // better node exists instead of blindly staying placed there.
                        if let Some((better, _)) = evaluate_cluster_rebalance_with(
                            nodes,
                            remembered,
                            workspace_name,
                            engine,
                            os,
                        )
                        .await
                        {
                            if let Some(path) = placement_file {
                                placement
                                    .workspaces
                                    .insert(workspace_name.to_string(), better);
                                save_placement(path, &placement);
                            }
                            return Ok(better);
                        }
                    }
                }
                if still_fits {
                    return Ok(remembered);
                }
            }
            // Node is absent or unviable: evict stale placement entry.
            if let Some(path) = placement_file {
                placement.workspaces.remove(workspace_name);
                save_placement(path, &placement);
            }
        }
        // Ask the cluster first: any live node knows (by gossip) who already holds the
        // workspace and who is quietest, and it can move an idle workspace off an
        // overloaded node.
        for seed in rendezvous_order(nodes, workspace_name) {
            let Ok(answer) = ask_placement(seed, workspace_name, engine, os).await else {
                continue;
            };
            // An older gateway ignores the OS the request names, so its choice is checked.
            if let Some(chosen) = answer
                .node
                .as_deref()
                .and_then(|a| a.parse::<SocketAddr>().ok())
                && node_fits(chosen, None, os).await
            {
                tracing::debug!(%chosen, reason = %answer.reason, "cluster placement");
                if let Some(path) = placement_file {
                    placement
                        .workspaces
                        .insert(workspace_name.to_string(), chosen);
                    save_placement(path, &placement);
                }
                return Ok(chosen);
            }
            break;
        }
        // Fallback without a cluster view: prefer the quietest, roomiest node (by congestion score) among
        // the ones that answer and can serve the engine, rendezvous order as tie-break.
        let mut candidates = Vec::new();
        let mut unsupported = Vec::new();
        let mut on_macos = Vec::new();
        let mut short = Vec::new();
        // Why the first node that did not answer did not (#340): a gateway that is down, or on
        // macOS an app not let onto the local network, reads differently.
        let mut first_failure = None;
        for candidate in rendezvous_order(nodes, workspace_name) {
            match node_status(candidate).await {
                Ok(status) if status_fits(&status, engine, os) => {
                    if runs_os(&status, "macos") {
                        on_macos.push(candidate);
                    }
                    if status.host.pressure().is_some() {
                        short.push(candidate);
                    }
                    candidates.push((candidate, status.congestion_score()));
                }
                Ok(_) => unsupported.push(candidate),
                Err(err) => {
                    // A node that accepts TCP but answers no status is only usable when
                    // nothing specific is required of it.
                    if engine.is_none() && os.is_none() && is_alive(candidate).await {
                        candidates.push((candidate, 500.0));
                    } else if first_failure.is_none() {
                        first_failure = Some(format!("{candidate}: {}", err.root_cause()));
                    }
                }
            }
        }
        // A macOS node is a developer's Mac: work that does not need macOS goes there only when
        // no other node can take it, however quiet the Mac is (#308).
        if os.is_none() && candidates.iter().any(|(c, _)| !on_macos.contains(c)) {
            candidates.retain(|(c, _)| !on_macos.contains(c));
        }
        // A node short of memory or disk only when every other one is too (#396).
        if candidates.iter().any(|(c, _)| !short.contains(c)) {
            candidates.retain(|(c, _)| !short.contains(c));
        }
        if let Some(chosen) = choose_best_node(&candidates) {
            if let Some(path) = placement_file {
                placement
                    .workspaces
                    .insert(workspace_name.to_string(), chosen);
                save_placement(path, &placement);
            }
            return Ok(chosen);
        }
        return Err(match (os, engine) {
            (Some(os), _) if unsupported.is_empty() => anyhow!(
                "no reachable gateway runs {} (none of {} answered)",
                os_name(os),
                listed(nodes)
            ),
            (Some(os), _) => anyhow!(
                "no reachable gateway runs {} (reachable without it: {})",
                os_name(os),
                listed(&unsupported)
            ),
            (None, Some(engine)) if !unsupported.is_empty() => anyhow!(
                "no reachable gateway serves {engine} (reachable without it: {}); add a node with the {engine} language server installed",
                listed(&unsupported)
            ),
            _ => match first_failure {
                Some(why) => {
                    let hint = if why.contains("No route to host")
                        || why.contains("os error 65")
                        || why.contains("Connection refused")
                    {
                        "; hint: if node is reachable via SSH, forward gateway port via 'ssh -NL 9400:localhost:9400 <node>' and specify '--remote 127.0.0.1:9400'"
                    } else {
                        ""
                    };
                    anyhow!("no gateway reachable among {} ({why}{hint})", listed(nodes))
                }
                None => anyhow!("no gateway reachable among {}", listed(nodes)),
            },
        });
    };
    let loopback: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    if *only == loopback
        && (!is_alive(*only).await || (engine.is_some() && !node_fits(*only, engine, os).await))
        && let Some(path) = placement_file
    {
        let placement = load_placement(path);
        if let Some(remembered) = placement.workspaces.get(workspace_name).copied()
            && remembered != loopback
            && node_fits(remembered, engine, os).await
        {
            return Ok(remembered);
        }
    }
    // One node is used without asking, unless the checkout needs an OS it has to be shown to
    // run: a build there would fail on headers that do not exist (#248).
    if let Some(os) = os
        && !node_fits(*only, None, Some(os)).await
    {
        return Err(anyhow!(
            "no reachable gateway runs {} (reachable without it: {only})",
            os_name(os)
        ));
    }
    if let Some(path) = placement_file {
        let mut placement = load_placement(path);
        placement
            .workspaces
            .insert(workspace_name.to_string(), *only);
        save_placement(path, &placement);
    }
    Ok(*only)
}
