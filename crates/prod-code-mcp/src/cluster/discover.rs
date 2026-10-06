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
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ClusterResponse, MetricsRequest, MetricsResponse, PlaceRequest, PlaceResponse, ProdCodeCodec,
    StatusResponse, WireMessage,
};
use serde::{Deserialize, Serialize};
use tokio_util::codec::Framed;

use super::placement::{load_placement, placement_path, save_placement};
use super::routing::PROBE_TIMEOUT;
use super::selection::is_alive;

/// Asks one node for the cluster as it sees it (gossip view).
pub async fn cluster_view(addr: SocketAddr) -> Result<ClusterResponse> {
    let stream = tokio::time::timeout(PROBE_TIMEOUT, prod_code_protocol::transport::connect(addr))
        .await
        .map_err(|_| anyhow!("connect timed out"))??;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed.send(WireMessage::ClusterRequest).await?;
    match tokio::time::timeout(Duration::from_secs(3), framed.next()).await {
        Ok(Some(Ok(WireMessage::ClusterResponse(view)))) => Ok(view),
        Ok(Some(Ok(other))) => Err(anyhow!("unexpected reply: {other:?}")),
        Ok(Some(Err(e))) => Err(anyhow!("decode error: {e}")),
        Ok(None) => Err(anyhow!("connection closed")),
        Err(_) => Err(anyhow!("cluster view timed out")),
    }
}

/// Asks one node where `workspace_name` (needing `engine`, and a node running `os`) should be
/// placed.
pub async fn ask_placement(
    addr: SocketAddr,
    workspace_name: &str,
    engine: Option<&str>,
    os: Option<&str>,
) -> Result<PlaceResponse> {
    ask_placement_opt(addr, workspace_name, engine, os, false).await
}

/// Asks one node where `workspace_name` should be placed, optionally rebalancing active workloads (Phase 5.3).
pub async fn ask_placement_opt(
    addr: SocketAddr,
    workspace_name: &str,
    engine: Option<&str>,
    os: Option<&str>,
    rebalance_active: bool,
) -> Result<PlaceResponse> {
    let stream = tokio::time::timeout(PROBE_TIMEOUT, prod_code_protocol::transport::connect(addr))
        .await
        .map_err(|_| anyhow!("connect timed out"))??;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::PlaceRequest(PlaceRequest {
            workspace_name: workspace_name.to_string(),
            engine: engine.map(String::from),
            os: os.map(String::from),
            rebalance_active,
        }))
        .await?;
    match tokio::time::timeout(Duration::from_secs(3), framed.next()).await {
        Ok(Some(Ok(WireMessage::PlaceResponse(resp)))) => Ok(resp),
        Ok(Some(Ok(other))) => Err(anyhow!("unexpected reply: {other:?}")),
        Ok(Some(Err(e))) => Err(anyhow!("decode error: {e}")),
        Ok(None) => Err(anyhow!("connection closed")),
        Err(_) => Err(anyhow!("placement timed out")),
    }
}

pub(crate) fn cluster_cache_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/prod_code/cluster.json"))
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct ClusterCache {
    #[serde(default)]
    pub(crate) nodes: Vec<String>,
}

/// The nodes of the cluster starting from the configured seeds: the first seed that
/// answers is asked for its gossip view and every live member is added; the result is
/// cached so a cluster whose seeds are down is still known. One seed address is enough.
pub async fn discover_nodes(seeds: &[SocketAddr]) -> Vec<SocketAddr> {
    discover_nodes_with_paths(
        seeds,
        placement_path().as_deref(),
        cluster_cache_path().as_deref(),
    )
    .await
}

pub async fn discover_nodes_with_paths(
    seeds: &[SocketAddr],
    placement_file: Option<&Path>,
    cache_file: Option<&Path>,
) -> Vec<SocketAddr> {
    // Fast path: UDP discovery (multicast + unicast probes, 250ms).
    let udp_nodes = tokio::task::spawn_blocking({
        let seeds = seeds.to_vec();
        move || prod_code_protocol::discovery::discover_addrs(&seeds)
    })
    .await
    .unwrap_or_default();

    let mut nodes: Vec<SocketAddr> = seeds.to_vec();
    for addr in &udp_nodes {
        if !nodes.contains(addr) {
            nodes.push(*addr);
        }
    }

    let mut probe_seeds: Vec<SocketAddr> = seeds.to_vec();
    let mut remembered_nodes: Vec<SocketAddr> = Vec::new();

    if let Some(path) = placement_file {
        let placement = load_placement(path);
        for addr in placement.workspaces.values() {
            if !remembered_nodes.contains(addr) {
                remembered_nodes.push(*addr);
            }
            if !probe_seeds.contains(addr) {
                probe_seeds.push(*addr);
            }
        }
    }
    if let Some(path) = cache_file
        && let Ok(bytes) = std::fs::read(path)
        && let Ok(cache) = serde_json::from_slice::<ClusterCache>(&bytes)
    {
        for n in cache.nodes {
            if let Ok(addr) = n.parse::<SocketAddr>()
                && !probe_seeds.contains(&addr)
            {
                probe_seeds.push(addr);
            }
        }
    }

    let mut learned = false;
    for seed in &probe_seeds {
        if let Ok(view) = cluster_view(*seed).await {
            for n in view.nodes.iter().filter(|n| n.alive) {
                if let Ok(addr) = n.addr.parse::<SocketAddr>()
                    && !nodes.contains(&addr)
                {
                    nodes.push(addr);
                }
            }
            learned = true;
            if remembered_nodes.iter().all(|r| nodes.contains(r)) {
                break;
            }
        }
    }

    for remembered in &remembered_nodes {
        if !nodes.contains(remembered) && is_alive(*remembered).await {
            nodes.push(*remembered);
        }
    }

    if learned && let Some(path) = placement_file {
        let mut placement = load_placement(path);
        let live_addrs: std::collections::HashSet<SocketAddr> = nodes.iter().copied().collect();
        let before = placement.workspaces.len();
        placement
            .workspaces
            .retain(|_, addr| live_addrs.contains(addr));
        if placement.workspaces.len() < before {
            save_placement(path, &placement);
        }
    }

    if let Some(path) = cache_file {
        if learned {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let cache = ClusterCache {
                nodes: nodes.iter().map(|n| n.to_string()).collect(),
            };
            if let Ok(bytes) = serde_json::to_vec_pretty(&cache) {
                let _ = std::fs::write(path, bytes);
            }
        } else {
            if let Ok(bytes) = std::fs::read(path)
                && let Ok(cache) = serde_json::from_slice::<ClusterCache>(&bytes)
            {
                for n in cache.nodes {
                    if let Ok(addr) = n.parse::<SocketAddr>()
                        && !nodes.contains(&addr)
                    {
                        nodes.push(addr);
                    }
                }
            }
            for addr in &remembered_nodes {
                if !nodes.contains(addr) {
                    nodes.push(*addr);
                }
            }
        }
    }
    nodes
}

/// Asks one node for its usage metrics over the last `since_secs` (0 = all it holds).
pub async fn node_metrics(addr: SocketAddr, since_secs: u64) -> Result<MetricsResponse> {
    let stream = tokio::time::timeout(PROBE_TIMEOUT, prod_code_protocol::transport::connect(addr))
        .await
        .map_err(|_| anyhow!("connect timed out"))??;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::MetricsRequest(MetricsRequest { since_secs }))
        .await?;
    match tokio::time::timeout(Duration::from_secs(10), framed.next()).await {
        Ok(Some(Ok(WireMessage::MetricsResponse(m)))) => Ok(m),
        Ok(Some(Ok(other))) => Err(anyhow!("unexpected reply: {other:?}")),
        Ok(Some(Err(e))) => Err(anyhow!("decode error: {e}")),
        Ok(None) => Err(anyhow!("connection closed")),
        Err(_) => Err(anyhow!("metrics timed out")),
    }
}

/// Asks one node for its status.
pub async fn node_status(addr: SocketAddr) -> Result<StatusResponse> {
    let stream = tokio::time::timeout(PROBE_TIMEOUT, prod_code_protocol::transport::connect(addr))
        .await
        .map_err(|_| anyhow!("connect timed out"))??;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed.send(WireMessage::StatusRequest).await?;
    match tokio::time::timeout(Duration::from_secs(3), framed.next()).await {
        Ok(Some(Ok(WireMessage::StatusResponse(status)))) => Ok(status),
        Ok(Some(Ok(other))) => Err(anyhow!("unexpected reply: {other:?}")),
        Ok(Some(Err(e))) => Err(anyhow!("decode error: {e}")),
        Ok(None) => Err(anyhow!("connection closed")),
        Err(_) => Err(anyhow!("status timed out")),
    }
}
