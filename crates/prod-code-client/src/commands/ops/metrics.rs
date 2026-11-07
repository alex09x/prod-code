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
    // count, errors, p50, p95, answers given before the engine was ready, and the empty ones.
    type Agg = (u64, u64, u64, u64, u64, u64);
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
            e.4 += q.before_ready;
            e.5 += q.empty_before_ready;
        }
    }
    if by_key.is_empty() {
        println!("no queries in the window");
    } else {
        println!(
            "{:<12} {:<18} {:<28} {:<34} {:>7} {:>5} {:>7} {:>7} {:>11}",
            "agent", "host", "workspace", "method", "count", "err", "p50ms", "p95ms", "early/empty"
        );
        for ((agent, host, ws, method), (count, errors, p50, p95, early, early_empty)) in &by_key {
            println!(
                "{:<12} {:<18} {:<28} {:<34} {:>7} {:>5} {:>7} {:>7} {:>11}",
                truncate(agent, 12),
                truncate(host, 18),
                truncate(ws, 28),
                truncate(method.trim_start_matches("textDocument/"), 34),
                count,
                errors,
                p50,
                p95,
                early_before_ready(*early, *early_empty)
            );
        }
        println!(
            "early/empty: answers from an index given while the engine was loading, indexing or not saying, and how many of them were empty; such an empty answer is not proven final"
        );
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
        if let Some(snap) = m.snapshots.last() {
            let cpu_str = snap
                .cpu_usage_pct()
                .map(|p| format!("{p:.1}%"))
                .unwrap_or_else(|| "-".to_string());
            let rss_str = snap
                .process_rss_bytes
                .map(|b| format!("{:.0} MB", b as f64 / 1_048_576.0))
                .unwrap_or_else(|| "-".to_string());
            let disk_str = snap
                .storage_free_millis
                .map(|m| format!("{}% free", m / 10))
                .unwrap_or_else(|| "-".to_string());
            println!(
                "  snapshot: CPU {cpu_str} | RSS {rss_str} | Disk {disk_str} | Sessions {} | Workspaces {} | Engines {}",
                snap.active_sessions, snap.workspace_count, snap.engine_count
            );
        }
        if let Some(ref inv) = m.inventory {
            let engines_str: Vec<String> = inv
                .engines
                .iter()
                .filter(|e| e.available)
                .map(|e| {
                    if let Some(tc) = e.toolchains.first() {
                        let short_ver = tc.version.split_whitespace().nth(1).unwrap_or(&tc.version);
                        format!("{}:{}", e.engine, short_ver)
                    } else {
                        e.engine.clone()
                    }
                })
                .collect();
            if !engines_str.is_empty() {
                println!("  toolchains: {}", engines_str.join(", "));
            }
        }
    }
    Ok(())
}

/// "12/9" for twelve answers given before the engine was ready, nine of them empty; "-" for none.
fn early_before_ready(early: u64, empty: u64) -> String {
    if early == 0 {
        "-".to_string()
    } else {
        format!("{early}/{empty}")
    }
}

#[cfg(test)]
mod tests {
    use super::early_before_ready;

    #[test]
    fn answers_before_the_engine_was_ready_show_as_early_over_empty() {
        assert_eq!(early_before_ready(0, 0), "-");
        assert_eq!(early_before_ready(12, 9), "12/9");
        assert_eq!(early_before_ready(3, 0), "3/0");
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}
