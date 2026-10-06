/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

/// The address this gateway advertises: the bind address when it names a host, otherwise
/// the IPv4 of the interface that routes to the first peer (or to a public address) plus
/// the bind port.
pub(crate) fn detect_advertise_addr(bind: SocketAddr, first_peer: Option<&str>) -> String {
    if !bind.ip().is_unspecified() {
        return bind.to_string();
    }
    let probe = first_peer
        .and_then(|p| p.parse::<SocketAddr>().ok())
        .unwrap_or_else(|| "8.8.8.8:80".parse().unwrap());
    let local_ip = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect(probe).and_then(|_| s.local_addr()))
        .map(|a| a.ip())
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    format!("{local_ip}:{}", bind.port())
}

/// Whether a status lists `engine` among its engines (entries look like `swift (sourcekit-lsp)`).
pub(crate) fn cluster_supports_engine(status: &StatusResponse, engine: &str) -> bool {
    status.detected_engines.iter().any(|e| {
        e == engine
            || e.strip_prefix(engine)
                .is_some_and(|rest| rest.starts_with(' '))
    })
}

/// Sends this node's heartbeat to every known peer every [`GOSSIP_PERIOD`] and absorbs the
/// heartbeats they answer with, so every node ends up with the same picture of the cluster.
pub(crate) async fn gossip_loop(state: Arc<ServerState>) {
    let mut unconfirmed_since: std::collections::HashMap<String, Instant> =
        std::collections::HashMap::new();

    loop {
        tokio::time::sleep(GOSSIP_PERIOD).await;

        // 1. Evict silent peers from cluster that haven't sent heartbeats in PEER_EVICT
        {
            let mut cluster = state.cluster.write().await;
            let mut evicted = Vec::new();
            cluster.retain(|addr, entry| {
                if entry.last_seen.elapsed() >= PEER_EVICT {
                    evicted.push(addr.clone());
                    false
                } else {
                    true
                }
            });
            if !evicted.is_empty() {
                let mut peers = state.peers.write().await;
                for dead in &evicted {
                    peers.remove(dead);
                    unconfirmed_since.remove(dead);
                }
                tracing::info!(evicted = ?evicted, "evicted silent peers from cluster");
            }
        }

        // 2. Evict unconfirmed transitive peers from state.peers that never connected within PEER_EVICT
        {
            let cluster = state.cluster.read().await;
            let mut peers = state.peers.write().await;
            let now = Instant::now();
            let mut dead_unconfirmed = Vec::new();
            for p in peers.iter() {
                if !cluster.contains_key(p) {
                    let first_seen = unconfirmed_since.entry(p.clone()).or_insert(now);
                    if now.duration_since(*first_seen) >= PEER_EVICT {
                        dead_unconfirmed.push(p.clone());
                    }
                } else {
                    unconfirmed_since.remove(p);
                }
            }
            for d in &dead_unconfirmed {
                peers.remove(d);
                unconfirmed_since.remove(d);
            }
            if !dead_unconfirmed.is_empty() {
                tracing::info!(evicted = ?dead_unconfirmed, "evicted unreachable transitive peers");
            }
        }

        let peers: Vec<String> = state.peers.read().await.iter().cloned().collect();
        if peers.is_empty() {
            continue;
        }
        let own = state.own_gossip().await;
        for peer in peers {
            let own = own.clone();
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                let reply = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                    let addr: SocketAddr = peer.parse().ok()?;
                    // Peers share the cluster's token with the clients (#402).
                    let stream = prod_code_protocol::transport::connect_stream_with_client_config(
                        addr,
                        state.auth_token.as_deref(),
                        state.client_tls.clone(),
                    )
                    .await
                    .ok()?;
                    let mut framed = Framed::new(stream, ProdCodeCodec::new());
                    framed.send(WireMessage::Gossip(own)).await.ok()?;
                    match framed.next().await {
                        Some(Ok(WireMessage::Gossip(g))) => Some(g),
                        _ => None,
                    }
                })
                .await
                .ok()
                .flatten();
                if let Some(gossip) = reply {
                    state.absorb_gossip(gossip).await;
                }
            });
        }
    }
}

/// UDP discovery: listens for probe packets (multicast and unicast), replies with the full
/// cluster view, and periodically announces this node on multicast.
pub(crate) async fn discovery_loop(state: Arc<ServerState>) {
    use prod_code_protocol::discovery;

    let sock = match discovery::bind_discovery_socket() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%e, "UDP discovery socket bind failed; discovery disabled");
            return;
        }
    };
    let tok_sock = match tokio::net::UdpSocket::from_std(sock) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%e, "failed to register discovery socket with tokio");
            return;
        }
    };
    tracing::info!(
        port = discovery::DISCOVERY_PORT,
        "UDP discovery listener started"
    );

    let mut announce_tick = tokio::time::interval(discovery::ANNOUNCE_PERIOD);
    let mut buf = vec![0u8; 4096];

    loop {
        tokio::select! {
            // Incoming datagram: probe or peer announce.
            result = tok_sock.recv_from(&mut buf) => {
                match result {
                    Ok((n, from)) => {
                        let token = state.auth_token.as_deref();
                        if let Some(probe_nonce) = discovery::inspect_probe(&buf[..n], token) {
                            let own_line = build_own_announce(&state, probe_nonce.as_deref()).await;
                            let cluster = state.cluster.read().await;
                            let peer_lines: Vec<String> = cluster
                                .values()
                                .filter(|e| e.last_seen.elapsed() < std::time::Duration::from_secs(30))
                                .map(|e| build_peer_announce(e, token, probe_nonce.as_deref()))
                                .collect();
                            drop(cluster);
                            let payload = discovery::build_reply(&own_line, &peer_lines);
                            let _ = tok_sock.send_to(&payload, from).await;
                        } else if let Some(dns_resp) = handle_gateway_dns_query(&buf[..n], &state).await {
                            let _ = tok_sock.send_to(&dns_resp, from).await;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        tracing::debug!(%e, "discovery recv error");
                    }
                }
            }
            // Periodic multicast announce: minimal announcement for LAN discovery privacy (Phase 5.6).
            _ = announce_tick.tick() => {
                let line = build_own_minimal_announce(&state).await;
                let payload = discovery::build_reply(&line, &[]);
                let _ = tok_sock.send_to(
                    &payload,
                    std::net::SocketAddrV4::new(discovery::MULTICAST_GROUP, discovery::DISCOVERY_PORT),
                ).await;
            }
        }
    }
}

/// Handles incoming DNS queries (*.code.internal, _prod-code._tcp) over UDP on port 9401 (Phase 5.2).
pub(crate) async fn handle_gateway_dns_query(buf: &[u8], state: &ServerState) -> Option<Vec<u8>> {
    use prod_code_protocol::discovery::{DiscoveredNode, LoadedWorkspace};
    use prod_code_protocol::dns::handle_dns_packet;

    let own_advertise = state.advertise.read().await.clone();
    let own_addr: SocketAddr = own_advertise.parse().ok()?;
    let own_engines: Vec<String> = state
        .advertised_engines()
        .iter()
        .map(|e| e.split(' ').next().unwrap_or(e).to_string())
        .collect();
    let status = state.status().await;
    let ws_summary = state.workspace_manager.loaded_summary().await;
    let own_workspaces: Vec<LoadedWorkspace> = ws_summary
        .into_iter()
        .map(|(name, engine, sessions)| LoadedWorkspace {
            name,
            engine,
            sessions: sessions as u32,
        })
        .collect();

    let mut nodes = vec![DiscoveredNode {
        addr: own_addr,
        engines: own_engines,
        rss_mb: status.memory_rss_bytes.unwrap_or(0) / (1024 * 1024),
        load_per_cpu: status.load_per_cpu().unwrap_or(0.0),
        cpus: status.cpu_count.unwrap_or(0) as u32,
        mem_total_mb: status.host.memory_total_bytes.unwrap_or(0) / (1024 * 1024),
        mem_avail_mb: status.host.memory_available_bytes.unwrap_or(0) / (1024 * 1024),
        sessions: state
            .active_sessions
            .load(std::sync::atomic::Ordering::Relaxed) as u32,
        workspaces: own_workspaces,
        nonce: None,
    }];

    let cluster = state.cluster.read().await;
    for entry in cluster.values() {
        if entry.last_seen.elapsed() < std::time::Duration::from_secs(30) {
            if let Ok(addr) = entry.gossip.addr.parse::<SocketAddr>() {
                let eng: Vec<String> = entry
                    .gossip
                    .status
                    .detected_engines
                    .iter()
                    .map(|en| en.split(' ').next().unwrap_or(en).to_string())
                    .collect();
                let peer_workspaces: Vec<LoadedWorkspace> = entry
                    .gossip
                    .workspaces
                    .iter()
                    .map(|w| LoadedWorkspace {
                        name: w.name.clone(),
                        engine: w.engine.clone(),
                        sessions: w.sessions as u32,
                    })
                    .collect();
                nodes.push(DiscoveredNode {
                    addr,
                    engines: eng,
                    rss_mb: entry.gossip.status.memory_rss_bytes.unwrap_or(0) / (1024 * 1024),
                    load_per_cpu: entry.gossip.status.load_per_cpu().unwrap_or(0.0),
                    cpus: entry.gossip.status.cpu_count.unwrap_or(0) as u32,
                    mem_total_mb: entry.gossip.status.host.memory_total_bytes.unwrap_or(0)
                        / (1024 * 1024),
                    mem_avail_mb: entry.gossip.status.host.memory_available_bytes.unwrap_or(0)
                        / (1024 * 1024),
                    sessions: entry
                        .gossip
                        .workspaces
                        .iter()
                        .map(|w| w.sessions as u32)
                        .sum(),
                    workspaces: peer_workspaces,
                    nonce: None,
                });
            }
        }
    }
    drop(cluster);

    nodes.sort_by_key(|n| n.addr);
    handle_dns_packet(buf, &nodes)
}

/// Build this node's minimal discovery announce line for privacy (endpoint + engines only).
pub(crate) async fn build_own_minimal_announce(state: &ServerState) -> String {
    use prod_code_protocol::discovery;
    let advertise = state.advertise.read().await.clone();
    let engines_csv = state
        .advertised_engines()
        .iter()
        .map(|e| e.split(' ').next().unwrap_or(e).to_string())
        .collect::<Vec<_>>()
        .join(",");
    discovery::format_minimal_node_line(&advertise, &engines_csv, state.auth_token.as_deref())
}

/// Build this node's discovery announce line with full routing metadata and optional challenge nonce echo.
pub(crate) async fn build_own_announce(state: &ServerState, nonce: Option<&str>) -> String {
    use prod_code_protocol::discovery;
    let advertise = state.advertise.read().await.clone();
    let engines_csv = state
        .advertised_engines()
        .iter()
        .map(|e| e.split(' ').next().unwrap_or(e).to_string())
        .collect::<Vec<_>>()
        .join(",");
    let status = state.status().await;
    let ws_summary = state.workspace_manager.loaded_summary().await;
    let workspaces: Vec<(String, String, u32)> = ws_summary
        .into_iter()
        .map(|(name, engine, sessions)| (name, engine, sessions as u32))
        .collect();
    discovery::format_node_line_with_nonce(
        &advertise,
        &engines_csv,
        status.memory_rss_bytes.unwrap_or(0) / (1024 * 1024),
        status.load_per_cpu().unwrap_or(0.0),
        status.cpu_count.unwrap_or(0) as u32,
        status.host.memory_total_bytes.unwrap_or(0) / (1024 * 1024),
        status.host.memory_available_bytes.unwrap_or(0) / (1024 * 1024),
        state
            .active_sessions
            .load(std::sync::atomic::Ordering::Relaxed) as u32,
        &workspaces,
        state.auth_token.as_deref(),
        nonce,
    )
}

/// Build a peer's discovery announce line from its gossip data with optional challenge nonce echo.
pub(crate) fn build_peer_announce(entry: &PeerEntry, token: Option<&str>, nonce: Option<&str>) -> String {
    use prod_code_protocol::discovery;
    let eng = entry
        .gossip
        .status
        .detected_engines
        .iter()
        .map(|en| en.split(' ').next().unwrap_or(en).to_string())
        .collect::<Vec<_>>()
        .join(",");
    let workspaces: Vec<(String, String, u32)> = entry
        .gossip
        .workspaces
        .iter()
        .map(|w| (w.name.clone(), w.engine.clone(), w.sessions as u32))
        .collect();
    discovery::format_node_line_with_nonce(
        &entry.gossip.addr,
        &eng,
        entry.gossip.status.memory_rss_bytes.unwrap_or(0) / (1024 * 1024),
        entry.gossip.status.load_per_cpu().unwrap_or(0.0),
        entry.gossip.status.cpu_count.unwrap_or(0) as u32,
        entry.gossip.status.host.memory_total_bytes.unwrap_or(0) / (1024 * 1024),
        entry.gossip.status.host.memory_available_bytes.unwrap_or(0) / (1024 * 1024),
        entry
            .gossip
            .workspaces
            .iter()
            .map(|w| w.sessions as u32)
            .sum::<u32>(),
        &workspaces,
        token,
        nonce,
    )
}

