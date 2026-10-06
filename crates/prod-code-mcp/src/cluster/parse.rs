/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::{SocketAddr, ToSocketAddrs};

use anyhow::{Result, anyhow};

use super::discover::{ClusterCache, cluster_cache_path};

/// Parses `host:port[,host:port...]` (spaces allowed) or `auto` into resolved addresses, in order.
///
/// Supports `*.code.internal` virtual domains (e.g. `shop.code.internal`), resolving them via
/// smart DNS and warm workspace mapping over discovered cluster nodes (Roadmap 5.2).
pub fn parse_remotes(spec: &str) -> Result<Vec<SocketAddr>> {
    let trimmed = spec.trim();
    if trimmed.eq_ignore_ascii_case("auto") {
        return resolve_auto_remotes();
    }
    let mut nodes = Vec::new();
    for item in trimmed.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if item.eq_ignore_ascii_case("auto") {
            let auto_nodes = resolve_auto_remotes()?;
            for a in auto_nodes {
                if !nodes.contains(&a) {
                    nodes.push(a);
                }
            }
            continue;
        }
        if prod_code_protocol::dns::is_code_internal_domain(item) {
            let discovered = discover_auto_nodes_sync();
            if let Some(resolved) =
                prod_code_protocol::dns::resolve_smart_domain(item, &discovered, 9400)
            {
                for a in resolved {
                    if !nodes.contains(&a) {
                        nodes.push(a);
                    }
                }
                continue;
            } else {
                return Err(anyhow!(
                    "cannot resolve internal domain {item}: project not found on cluster nodes"
                ));
            }
        }
        let addrs: Vec<SocketAddr> = match item.to_socket_addrs() {
            Ok(parsed) => parsed.collect(),
            Err(_) => match (item, 9400).to_socket_addrs() {
                Ok(parsed) => parsed.collect(),
                Err(err) => {
                    return Err(anyhow!("cannot resolve gateway address {item}: {err}"));
                }
            },
        };
        let addr = addrs
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("gateway address {item} resolved to nothing"))?;
        if !nodes.contains(&addr) {
            nodes.push(addr);
        }
    }
    if nodes.is_empty() {
        return Err(anyhow!("no gateway addresses given"));
    }
    Ok(nodes)
}

/// Discovers live cluster nodes synchronously using active UDP probes and cached gossip.
pub fn discover_auto_nodes_sync() -> Vec<prod_code_protocol::discovery::DiscoveredNode> {
    let mut seeds = Vec::new();
    if let Some(path) = cluster_cache_path()
        && let Ok(bytes) = std::fs::read(&path)
        && let Ok(cache) = serde_json::from_slice::<ClusterCache>(&bytes)
    {
        for n in cache.nodes {
            if let Ok(addr) = n.parse::<SocketAddr>() {
                if !seeds.contains(&addr) {
                    seeds.push(addr);
                }
            }
        }
    }
    for env_var in &["PROD_CODE_SEEDS", "PROD_CODE_CLUSTER"] {
        if let Ok(seeds_spec) = std::env::var(env_var) {
            let spec = seeds_spec.trim();
            if !spec.eq_ignore_ascii_case("auto")
                && !spec.eq_ignore_ascii_case("10g")
                && !spec.eq_ignore_ascii_case("true")
                && !spec.eq_ignore_ascii_case("1")
                && !spec.eq_ignore_ascii_case("lan")
            {
                for s in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    if let Ok(addrs) = s.to_socket_addrs() {
                        for addr in addrs {
                            if !seeds.contains(&addr) {
                                seeds.push(addr);
                            }
                        }
                    }
                }
            }
        }
    }

    let mut discovered = prod_code_protocol::discovery::discover(&seeds);

    // Also populate stub DiscoveredNode for any cached nodes that didn't reply to UDP in 250ms
    for seed in seeds {
        if !discovered.iter().any(|d| d.addr == seed) {
            discovered.push(prod_code_protocol::discovery::DiscoveredNode {
                addr: seed,
                engines: Vec::new(),
                rss_mb: 0,
                load_per_cpu: 0.0,
                cpus: 1,
                mem_total_mb: 0,
                mem_avail_mb: 0,
                sessions: 0,
                workspaces: Vec::new(),
                nonce: None,
            });
        }
    }

    discovered
}

/// Resolves cluster seed addresses automatically from UDP multicast discovery,
/// cached cluster gossip, environment variables (PROD_CODE_CLUSTER, PROD_CODE_SEEDS),
/// or loopback gateway (Roadmap 5.2).
pub fn resolve_auto_remotes() -> Result<Vec<SocketAddr>> {
    let mut nodes = Vec::new();
    let discovered = discover_auto_nodes_sync();
    for d in discovered {
        if !nodes.contains(&d.addr) {
            nodes.push(d.addr);
        }
    }
    // Fallback to default loopback gateway address
    let loopback: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    if !nodes.contains(&loopback) {
        nodes.push(loopback);
    }
    Ok(nodes)
}
