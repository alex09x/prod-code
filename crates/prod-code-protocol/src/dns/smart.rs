/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{
    CODE_INTERNAL_ROOT, CODE_INTERNAL_SUFFIX, DnsAnswer, DnsQueryType, DnsRecordData, DnsSrvRecord,
    SRV_SERVICE_DOMAIN,
};
use super::wire::{format_dns_response, parse_dns_query};
use crate::discovery::DiscoveredNode;
use crate::messages::content_hash;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Checks whether a hostname or domain string targets the exact internal SRV service domain.
pub fn is_srv_service_domain(domain: &str) -> bool {
    let lower = domain.trim().to_ascii_lowercase();
    let host = lower.split(':').next().unwrap_or(&lower);
    host == SRV_SERVICE_DOMAIN || host == "_prod-code._tcp.code.local"
}

/// Checks whether a hostname or domain string targets the `.code.internal` virtual domain.
pub fn is_code_internal_domain(domain: &str) -> bool {
    let lower = domain.trim().to_ascii_lowercase();
    let host = lower.split(':').next().unwrap_or(&lower);
    host == CODE_INTERNAL_ROOT
        || host.ends_with(CODE_INTERNAL_SUFFIX)
        || host.ends_with(".code.local")
}

/// Extracts the target project name from a `.code.internal` domain name.
/// E.g. "shop.code.internal" -> Some("shop"), "SHOP.CODE.INTERNAL:9400" -> Some("shop"),
/// "cluster.code.internal" -> Some("cluster").
pub fn extract_project_name(domain: &str) -> Option<String> {
    let trimmed = domain.trim();
    let host = trimmed.split(':').next().unwrap_or(trimmed);
    let lower = host.to_ascii_lowercase();
    if lower == CODE_INTERNAL_ROOT {
        return Some("cluster".to_string());
    }
    if let Some(prefix) = lower.strip_suffix(CODE_INTERNAL_SUFFIX) {
        return Some(prefix.to_string());
    }
    if let Some(prefix) = lower.strip_suffix(".code.local") {
        return Some(prefix.to_string());
    }
    None
}

/// Maps a project name to its designated server node using warm workspace inspection
/// and deterministic rendezvous hashing (Phase 5.2).
pub fn resolve_project_node<'a>(
    project: &str,
    nodes: &'a [DiscoveredNode],
) -> Option<&'a DiscoveredNode> {
    if nodes.is_empty() {
        return None;
    }
    let proj_clean = project.trim();

    // 1. Prefer a node that already has the exact workspace loaded
    let warm_node = nodes.iter().find(|n| {
        n.workspaces
            .iter()
            .any(|w| w.name.eq_ignore_ascii_case(proj_clean))
    });
    if let Some(n) = warm_node {
        return Some(n);
    }

    // 2. Prefer a node that has this project or one of its named worktree copies.
    let project_lower = proj_clean.to_ascii_lowercase();
    let worktree_prefix = format!("{project_lower}--wt-");
    let prefix_node = nodes.iter().find(|n| {
        n.workspaces.iter().any(|w| {
            let name = w.name.to_ascii_lowercase();
            name == project_lower || name.starts_with(&worktree_prefix)
        })
    });
    if let Some(n) = prefix_node {
        return Some(n);
    }

    // 3. Fallback to deterministic rendezvous hashing over available nodes
    nodes.iter().max_by_key(|n| {
        // Match MCP placement's rendezvous key: `{workspace_name}|{socket_addr}`.
        let key = format!("{project_lower}|{}", n.addr);
        content_hash(key.as_bytes())
    })
}

/// Resolves a domain string (such as `shop.code.internal` or `cluster.code.internal:9400`)
/// to physical `SocketAddr` endpoints using cluster discovery and DNS mapping.
pub fn resolve_smart_domain(
    domain: &str,
    nodes: &[DiscoveredNode],
    default_port: u16,
) -> Option<Vec<SocketAddr>> {
    let trimmed = domain.trim();
    let (host, port) = match trimmed.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().unwrap_or(default_port)),
        None => (trimmed, default_port),
    };

    if !is_code_internal_domain(host) {
        return None;
    }

    if nodes.is_empty() {
        // If no nodes discovered yet, fallback to loopback with requested port
        return Some(vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)]);
    }

    let mut sorted_nodes = nodes.to_vec();
    sorted_nodes.sort_by_key(|n| n.addr);

    let project_owned = extract_project_name(host);
    let project = project_owned.as_deref().unwrap_or(host);

    if project.eq_ignore_ascii_case("cluster")
        || project.eq_ignore_ascii_case("all")
        || is_srv_service_domain(host)
    {
        // Return all known nodes, using node's own port if port was default
        let addrs: Vec<SocketAddr> = sorted_nodes
            .iter()
            .map(|n| {
                let p = if port == default_port {
                    n.addr.port()
                } else {
                    port
                };
                SocketAddr::new(n.addr.ip(), p)
            })
            .collect();
        return Some(addrs);
    }

    // 1. Direct match for node target names (e.g. "node-192-168-2-10", "node-192-168-2-10-9400")
    for node in &sorted_nodes {
        let ip_slug = node.addr.ip().to_string().replace('.', "-");
        let port_slug = format!("node-{ip_slug}-{}", node.addr.port());
        let standard_slug = format!("node-{ip_slug}");
        if project.eq_ignore_ascii_case(&port_slug)
            || project.eq_ignore_ascii_case(&standard_slug)
            || project.eq_ignore_ascii_case(&ip_slug)
        {
            let p = if port == default_port {
                node.addr.port()
            } else {
                port
            };
            return Some(vec![SocketAddr::new(node.addr.ip(), p)]);
        }
    }

    // 2. Also support 1-based index targets: "node-1", "node-2"
    if let Some(idx) = project
        .strip_prefix("node-")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&idx| idx >= 1 && idx <= sorted_nodes.len())
    {
        let node = &sorted_nodes[idx - 1];
        let p = if port == default_port {
            node.addr.port()
        } else {
            port
        };
        return Some(vec![SocketAddr::new(node.addr.ip(), p)]);
    }

    // 3. Resolve designated project node via warm check & rendezvous hashing
    if let Some(node) = resolve_project_node(project, &sorted_nodes) {
        let p = if port == default_port {
            node.addr.port()
        } else {
            port
        };
        return Some(vec![SocketAddr::new(node.addr.ip(), p)]);
    }

    None
}

/// Generates dynamic DNS SRV records for all active cluster daemon instances (RFC 2782).
pub fn generate_srv_records(nodes: &[DiscoveredNode]) -> Vec<DnsSrvRecord> {
    let mut sorted_nodes = nodes.to_vec();
    sorted_nodes.sort_by_key(|n| n.addr);

    let mut records = Vec::with_capacity(sorted_nodes.len());
    for node in &sorted_nodes {
        // Priority: lower number = higher priority. Scale with load: nodes with low load get priority 10.
        let priority = if node.load_per_cpu < 0.5 { 10 } else { 20 };
        // Weight: proportional to available memory (min 10)
        let weight = ((node.mem_avail_mb / 1024).clamp(10, 1000)) as u16;
        let ip_slug = node.addr.ip().to_string().replace('.', "-");
        let target = if node.addr.port() == 9400 {
            format!("node-{ip_slug}.code.internal")
        } else {
            format!("node-{ip_slug}-{}.code.internal", node.addr.port())
        };
        records.push(DnsSrvRecord {
            priority,
            weight,
            port: node.addr.port(),
            target,
        });
    }
    records
}

/// Handles an incoming DNS query over UDP, resolving `*.code.internal` or SRV records
/// and generating a standard DNS response packet.
pub fn handle_dns_packet(buf: &[u8], nodes: &[DiscoveredNode]) -> Option<Vec<u8>> {
    let (id, question) = parse_dns_query(buf).ok()?;
    if !is_code_internal_domain(&question.name) {
        return None;
    }

    let mut answers = Vec::new();
    let ttl = 60; // 60s TTL for dynamic discovery

    let domain_exists = if is_srv_service_domain(&question.name) {
        true
    } else {
        resolve_smart_domain(&question.name, nodes, 9400).is_some()
    };

    if domain_exists {
        match question.qtype {
            DnsQueryType::A | DnsQueryType::ANY => {
                if let Some(addrs) = resolve_smart_domain(&question.name, nodes, 9400) {
                    for addr in addrs {
                        if let IpAddr::V4(ipv4) = addr.ip() {
                            answers.push(DnsAnswer {
                                name: question.name.clone(),
                                ttl,
                                data: DnsRecordData::A(ipv4),
                            });
                        }
                    }
                }
            }
            DnsQueryType::SRV => {
                if is_srv_service_domain(&question.name)
                    || question.name.eq_ignore_ascii_case("cluster.code.internal")
                    || question.name.eq_ignore_ascii_case(CODE_INTERNAL_ROOT)
                {
                    let srvs = generate_srv_records(nodes);
                    for srv in srvs {
                        answers.push(DnsAnswer {
                            name: question.name.clone(),
                            ttl,
                            data: DnsRecordData::SRV(srv),
                        });
                    }
                }
            }
            DnsQueryType::TXT => {
                let count = nodes.len();
                let txt = format!("prod-code-cluster: nodes={count} domain=code.internal");
                answers.push(DnsAnswer {
                    name: question.name.clone(),
                    ttl,
                    data: DnsRecordData::TXT(txt),
                });
            }
            _ => {
                // Unsupported query type (e.g. AAAA = 28): return NOERROR with 0 answers (NODATA)
            }
        }
        Some(format_dns_response(id, &question, &answers, true, 0))
    } else {
        // Name does not exist: return NXDOMAIN (RCODE = 3)
        Some(format_dns_response(id, &question, &answers, true, 3))
    }
}
