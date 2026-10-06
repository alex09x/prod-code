/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::net::SocketAddr;

/// Display aggregated metrics from cluster nodes over a time window.
pub async fn run_metrics(nodes: &[SocketAddr], since: u64, json: bool) -> Result<()> {
    let mut all = Vec::new();
    for node in nodes {
        match prod_code_mcp::cluster::node_metrics(*node, since).await {
            Ok(m) => all.push(m),
            Err(e) => eprintln!("{node}: {e}"),
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(());
    }
    let window = if since == 0 {
        "all in memory".to_string()
    } else {
        format!("last {}h{:02}m", since / 3600, (since % 3600) / 60)
    };
    println!("⚡ prod-code usage ({window}, {} node(s))", all.len());
    println!("────────────────────────────────────────────────────────────────────────");
    // Queries: by agent → workspace → method, summed over nodes.
    type Key = (String, String, String, String);
    type Agg = (u64, u64, u64, u64);
    let mut by_key: std::collections::BTreeMap<Key, Agg> = std::collections::BTreeMap::new();
    for m in &all {
        for q in &m.queries {
            let e = by_key
                .entry((
                    q.agent.clone(),
                    q.host.clone(),
                    q.workspace.clone(),
                    q.method.clone(),
                ))
                .or_default();
            e.0 += q.count;
            e.1 += q.errors;
            e.2 = e.2.max(q.p50_ms);
            e.3 = e.3.max(q.p95_ms);
        }
    }
    if by_key.is_empty() {
        println!("no queries in the window");
    } else {
        println!(
            "{:<12} {:<18} {:<28} {:<34} {:>7} {:>5} {:>7} {:>7}",
            "agent", "host", "workspace", "method", "count", "err", "p50ms", "p95ms"
        );
        for ((agent, host, ws, method), (count, errors, p50, p95)) in &by_key {
            println!(
                "{:<12} {:<18} {:<28} {:<34} {:>7} {:>5} {:>7} {:>7}",
                truncate(agent, 12),
                truncate(host, 18),
                truncate(ws, 28),
                truncate(method.trim_start_matches("textDocument/"), 34),
                count,
                errors,
                p50,
                p95
            );
        }
    }
    let mut execs: Vec<_> = all.iter().flat_map(|m| m.execs.iter().cloned()).collect();
    if !execs.is_empty() {
        execs.sort_by_key(|e| std::cmp::Reverse(e.total_ms));
        println!("────────────────────────────────────────────────────────────────────────");
        println!(
            "{:<12} {:<18} {:<28} {:<40} {:>5} {:>4} {:>8}",
            "agent", "host", "workspace", "command", "runs", "fail", "total s"
        );
        for e in execs.iter().take(30) {
            println!(
                "{:<12} {:<18} {:<28} {:<40} {:>5} {:>4} {:>8.1}",
                truncate(&e.agent, 12),
                truncate(&e.host, 18),
                truncate(&e.workspace, 28),
                truncate(&e.command, 40),
                e.count,
                e.failures,
                e.total_ms as f64 / 1000.0
            );
        }
    }
    println!("────────────────────────────────────────────────────────────────────────");
    for m in &all {
        println!(
            "{:<22} syncs {:>5}  files {:>7}  {:>8.1} MB  events in memory {}",
            m.node,
            m.sync_rounds,
            m.sync_files,
            m.sync_bytes as f64 / 1_048_576.0,
            m.events_in_memory
        );
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}
