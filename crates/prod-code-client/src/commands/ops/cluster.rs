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
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use std::net::SocketAddr;
use tokio_util::codec::Framed;

/// A gateway's status as one JSON object for scripts (#398): the fields as the gateway sent
/// them, plus the address asked, the round trip, and whether the node is healthy and if not why.
pub fn status_snapshot(
    remote: SocketAddr,
    rtt: std::time::Duration,
    status: &prod_code_protocol::StatusResponse,
) -> serde_json::Value {
    let mut snapshot = serde_json::to_value(status).unwrap_or_default();
    if let Some(object) = snapshot.as_object_mut() {
        let pressure = status.host.pressure();
        object.insert("remote".into(), serde_json::json!(remote.to_string()));
        object.insert(
            "rtt_ms".into(),
            serde_json::json!(rtt.as_secs_f64() * 1000.0),
        );
        object.insert("healthy".into(), serde_json::json!(pressure.is_none()));
        object.insert("pressure".into(), serde_json::json!(pressure));
        object.insert(
            "congestion_score".into(),
            serde_json::json!(status.congestion_score()),
        );
    }
    snapshot
}

/// Query remote gateway for health and status snapshot.
pub async fn run_status_probe(
    remote: SocketAddr,
    json: bool,
    fallback_note: Option<String>,
) -> Result<()> {
    let start = std::time::Instant::now();
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to prod-code gateway at {remote}"))?;
    let rtt = start.elapsed();

    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed.send(WireMessage::StatusRequest).await?;

    if let Some(msg) = framed.next().await {
        match msg? {
            WireMessage::StatusResponse(resp) if json => {
                let mut snapshot = status_snapshot(remote, rtt, &resp);
                if let Some(note) = &fallback_note
                    && let Some(obj) = snapshot.as_object_mut()
                {
                    obj.insert("fallback_transport".to_string(), serde_json::json!(note));
                }
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            }
            WireMessage::StatusResponse(resp) => {
                let hours = resp.uptime_seconds / 3600;
                let minutes = (resp.uptime_seconds % 3600) / 60;
                let seconds = resp.uptime_seconds % 60;

                println!("⚡ prod-code Remote Code Intelligence Gateway");
                println!("────────────────────────────────────────────────────");
                let note_suffix = fallback_note
                    .as_deref()
                    .map(|n| format!(" [{n}]"))
                    .unwrap_or_default();
                println!("Remote Address:    {remote} ({:.2?} RTT){note_suffix}", rtt);
                if let Some(v) = &resp.version {
                    let commit_suffix = resp
                        .git_commit
                        .as_deref()
                        .map(|c| format!(" (commit {c})"))
                        .unwrap_or_default();
                    println!("Gateway Version:   {}{}", v, commit_suffix);
                } else if let Some(c) = &resp.git_commit {
                    println!("Gateway Commit:    {}", c);
                }
                println!("Server PID:        {}", resp.server_pid);
                println!("Uptime:            {}h {}m {}s", hours, minutes, seconds);
                if let Some(mb) = resp.memory_rss_mb() {
                    println!("Memory RSS:        {:.2} MB", mb);
                }
                println!("Active Sessions:   {}", resp.active_sessions);
                println!("Running Commands:  {}", resp.running_commands.len());
                for line in resp.running_lines() {
                    println!("  • {line}");
                }
                println!("Loaded Workspaces: {}", resp.loaded_workspaces);
                println!(
                    "Queries Handled:   {} (in-flight: {})",
                    resp.total_queries, resp.active_queries
                );
                println!("Engines Available: {}", resp.detected_engines.join(", "));
                let host = resp.host.describe();
                if !host.is_empty() {
                    println!("Host Resources:    {host}");
                }
                match resp.host.pressure() {
                    Some(why) => println!(
                        "Status:            SHORT ({why}): new workspaces go to other nodes"
                    ),
                    None => println!("Status:            HEALTHY"),
                }
            }
            WireMessage::Disconnect { reason } => {
                anyhow::bail!("the gateway at {remote} closed the connection: {reason}")
            }
            other => anyhow::bail!("Unexpected response from gateway: {:?}", other),
        }
    } else {
        anyhow::bail!("Gateway closed connection without responding");
    }

    Ok(())
}

/// answers, each configured node's status snapshot or the error it gave, and where the checkout
/// is placed.
pub async fn cluster_snapshot(
    nodes: &[SocketAddr],
    workspace_name: &str,
    engine: Option<&str>,
    required_os: Option<&str>,
    rebalance: bool,
) -> serde_json::Value {
    let mut gossip = serde_json::Value::Null;
    for node in nodes {
        if let Ok(view) = prod_code_mcp::cluster::cluster_view(*node).await {
            gossip = serde_json::to_value(view).unwrap_or_default();
            break;
        }
    }
    let mut listed = Vec::new();
    for node in nodes {
        let started = std::time::Instant::now();
        listed.push(match prod_code_mcp::cluster::node_status(*node).await {
            Ok(status) => {
                let mut snapshot = status_snapshot(*node, started.elapsed(), &status);
                snapshot["up"] = serde_json::json!(true);
                snapshot
            }
            Err(e) => serde_json::json!({
                "remote": node.to_string(),
                "up": false,
                "error": format!("{e:#}"),
            }),
        });
    }
    let mut rebalance_info = None;
    if rebalance {
        for seed in prod_code_mcp::cluster::rendezvous_order(nodes, workspace_name) {
            if let Ok(resp) = prod_code_mcp::cluster::ask_placement_opt(
                seed,
                workspace_name,
                engine,
                required_os,
                true,
            )
            .await
            {
                if let Some(target) = resp
                    .node
                    .as_deref()
                    .and_then(|a| a.parse::<SocketAddr>().ok())
                {
                    prod_code_mcp::cluster::remember_placement(workspace_name, target);
                    rebalance_info = Some(serde_json::json!({
                        "target": target.to_string(),
                        "reason": resp.reason,
                    }));
                    break;
                }
            }
        }
    }
    let home = prod_code_mcp::cluster::rendezvous_order(nodes, workspace_name)
        .first()
        .map(|n| n.to_string());
    let remembered = prod_code_mcp::cluster::remembered_node(workspace_name);
    let placed_on_is_cached = remembered.is_some_and(|r| !nodes.contains(&r));
    serde_json::json!({
        "gossip": gossip,
        "nodes": listed,
        "workspace": workspace_name,
        "engine": engine,
        "home": home,
        "placed_on": remembered.map(|n| n.to_string()),
        "placed_on_is_cached": placed_on_is_cached,
        "rebalanced": rebalance_info,
    })
}

/// Show every gateway node and the placement of the current checkout.
pub async fn run_cluster(
    nodes: &[SocketAddr],
    workspace_name: &str,
    engine: Option<&str>,
    workspace_root: Option<&std::path::Path>,
    json: bool,
    rebalance: bool,
) -> Result<()> {
    let required_os = if engine == Some("go") {
        workspace_root
            .and_then(prod_code_mcp::sync::macos_only_cgo)
            .map(|_| "macos")
    } else {
        None
    };
    if json {
        let snapshot =
            cluster_snapshot(nodes, workspace_name, engine, required_os, rebalance).await;
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
        return Ok(());
    }
    let mut rebalanced_target = None;
    if rebalance {
        for seed in prod_code_mcp::cluster::rendezvous_order(nodes, workspace_name) {
            if let Ok(resp) = prod_code_mcp::cluster::ask_placement_opt(
                seed,
                workspace_name,
                engine,
                required_os,
                true,
            )
            .await
            {
                if let Some(target) = resp
                    .node
                    .as_deref()
                    .and_then(|a| a.parse::<SocketAddr>().ok())
                {
                    prod_code_mcp::cluster::remember_placement(workspace_name, target);
                    rebalanced_target = Some((target, resp.reason));
                    break;
                }
            }
        }
    }
    println!("⚡ prod-code cluster ({} node(s))", nodes.len());
    println!("────────────────────────────────────────────────────");
    // The gossip view of the first node that answers: what every node holds.
    for node in nodes {
        if let Ok(view) = prod_code_mcp::cluster::cluster_view(*node).await {
            println!("gossip view from {}:", view.this_node);
            for peer in &view.nodes {
                let ws: Vec<String> = peer
                    .workspaces
                    .iter()
                    .map(|w| format!("{}[{}:{}]", w.name, w.engine, w.sessions))
                    .collect();
                println!(
                    "  {:<22} {:<5} load {:>5.2}/cpu  score {:>5.2}  seen {:>3}s ago  {}",
                    peer.addr,
                    if peer.alive { "UP" } else { "STALE" },
                    peer.status.load_per_cpu().unwrap_or(0.0),
                    peer.status.congestion_score(),
                    peer.last_seen_secs,
                    if ws.is_empty() {
                        "-".to_string()
                    } else {
                        ws.join(" ")
                    }
                );
                if let Some(why) = peer.status.host.pressure() {
                    println!("  {:<22} short: {why}", "");
                }
            }
            println!("────────────────────────────────────────────────────");
            break;
        }
    }
    let home = prod_code_mcp::cluster::rendezvous_order(nodes, workspace_name)
        .first()
        .copied();
    for node in nodes {
        let started = std::time::Instant::now();
        match prod_code_mcp::cluster::node_status(*node).await {
            Ok(status) => {
                println!(
                    "{node:<22} UP    {:>6.2} ms  score {:>5.2}  load {:>5.2}/cpu ({} cpus)  uptime {}h{:02}m  workspaces {}  sessions {}  commands {}  rss {:.0} MB",
                    started.elapsed().as_secs_f64() * 1000.0,
                    status.congestion_score(),
                    status.load_per_cpu().unwrap_or(0.0),
                    status.cpu_count.unwrap_or(0),
                    status.uptime_seconds / 3600,
                    (status.uptime_seconds % 3600) / 60,
                    status.loaded_workspaces,
                    status.active_sessions,
                    status.running_commands.len(),
                    status.memory_rss_mb().unwrap_or(0.0)
                );
                let engines: Vec<&str> = status
                    .detected_engines
                    .iter()
                    .filter(|e| e.as_str() != "generic-lsp")
                    .map(|e| e.split(' ').next().unwrap_or(e))
                    .collect();
                let ver_suffix = match (&status.version, &status.git_commit) {
                    (Some(v), Some(c)) => format!(" (prod-code {v}, commit {c})"),
                    (Some(v), None) => format!(" (prod-code {v})"),
                    (None, Some(c)) => format!(" (commit {c})"),
                    (None, None) => String::new(),
                };
                println!("{:<22} engines: {}{}", "", engines.join(", "), ver_suffix);
                let host = status.host.describe();
                if !host.is_empty() {
                    match status.host.pressure() {
                        Some(_) => {
                            println!("{:<22} host: {host} — short, takes no new workspaces", "")
                        }
                        None => println!("{:<22} host: {host}", ""),
                    }
                }
            }
            Err(e) => println!("{node:<22} DOWN  {e}"),
        }
    }
    println!("────────────────────────────────────────────────────");
    println!("Workspace:           {workspace_name}");
    println!("Engine needed:       {}", engine.unwrap_or("(any)"));
    if let Some(home) = home {
        println!("Home node (hash):    {home}");
    }
    let remembered = prod_code_mcp::cluster::remembered_node(workspace_name);
    if let Some((target, ref reason)) = rebalanced_target {
        println!("Rebalance:           Active workload rebalanced to {target} ({reason})");
    }
    match remembered {
        Some(node) if nodes.contains(&node) => println!("Placed on:           {node}"),
        Some(node) => println!("Placed on:           {node} (cached placement)"),
        None => println!("Placed on:           (not yet)"),
    }
    Ok(())
}
