//! Multiple gateways (Phase 5.1, client side): a workspace is placed on one node by
//! rendezvous hashing over the reachable nodes, the placement is remembered locally so all
//! sessions of that checkout keep hitting the node whose engine and build cache are warm, and
//! a dead node fails over to the next one.

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, StatusResponse, WireMessage, content_hash};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

/// How long a node has to accept a TCP connection before it counts as down.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(400);

/// Parses `host:port[,host:port...]` (spaces allowed) into resolved addresses, in order.
pub fn parse_remotes(spec: &str) -> Result<Vec<SocketAddr>> {
    let mut nodes = Vec::new();
    for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let addr = item
            .to_socket_addrs()
            .with_context(|| format!("cannot resolve gateway address {item}"))?
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

/// Nodes ordered by rendezvous (highest-random-weight) hashing for `workspace_name`: the
/// first entry is the workspace's home node; the order is stable across clients and only the
/// affected workspaces move when a node is added or removed.
pub fn rendezvous_order(nodes: &[SocketAddr], workspace_name: &str) -> Vec<SocketAddr> {
    let mut scored: Vec<(u64, SocketAddr)> = nodes
        .iter()
        .map(|addr| {
            let key = format!("{workspace_name}|{addr}");
            (content_hash(key.as_bytes()), *addr)
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, addr)| addr).collect()
}

/// Whether `addr` accepts a TCP connection within [`PROBE_TIMEOUT`].
pub async fn is_alive(addr: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Placement {
    #[serde(default)]
    workspaces: BTreeMap<String, SocketAddr>,
}

fn placement_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/prod_code/placement.json"))
}

fn load_placement(path: &Path) -> Placement {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_placement(path: &Path, placement: &Placement) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(placement) {
        let _ = std::fs::write(path, bytes);
    }
}

/// Chooses the gateway for `workspace_name` among `nodes`: the remembered placement when it
/// is still one of the nodes and alive, otherwise the first alive node in rendezvous order,
/// which is then remembered. A single node is returned as is.
pub async fn pick_node(nodes: &[SocketAddr], workspace_name: &str) -> Result<SocketAddr> {
    pick_node_with(nodes, workspace_name, placement_path().as_deref()).await
}

pub async fn pick_node_with(
    nodes: &[SocketAddr],
    workspace_name: &str,
    placement_file: Option<&Path>,
) -> Result<SocketAddr> {
    let [only] = nodes else {
        if nodes.is_empty() {
            return Err(anyhow!("no gateway addresses given"));
        }
        let mut placement = placement_file.map(load_placement).unwrap_or_default();
        if let Some(remembered) = placement.workspaces.get(workspace_name).copied()
            && nodes.contains(&remembered)
            && is_alive(remembered).await
        {
            return Ok(remembered);
        }
        // First placement: prefer the quietest node (load per CPU) among the ones that
        // answer, keeping rendezvous order as the tie-break and the fallback.
        let mut candidates = Vec::new();
        for candidate in rendezvous_order(nodes, workspace_name) {
            match node_status(candidate).await {
                Ok(status) => candidates.push((candidate, status.load_per_cpu())),
                Err(_) => {
                    if is_alive(candidate).await {
                        candidates.push((candidate, None));
                    }
                }
            }
        }
        if let Some(chosen) = choose_quietest(&candidates) {
            if let Some(path) = placement_file {
                placement
                    .workspaces
                    .insert(workspace_name.to_string(), chosen);
                save_placement(path, &placement);
            }
            return Ok(chosen);
        }
        return Err(anyhow!(
            "no gateway reachable among {}",
            nodes
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };
    Ok(*only)
}

/// Among alive candidates in rendezvous order with their load per CPU, the quietest one;
/// nodes without a load figure rank after those with one, and ties keep rendezvous order.
pub fn choose_quietest(candidates: &[(SocketAddr, Option<f64>)]) -> Option<SocketAddr> {
    candidates
        .iter()
        .enumerate()
        .min_by(|(ia, (_, la)), (ib, (_, lb))| {
            let key = |load: &Option<f64>| load.map(|l| (l * 1000.0) as i64).unwrap_or(i64::MAX);
            key(la).cmp(&key(lb)).then(ia.cmp(ib))
        })
        .map(|(_, (addr, _))| *addr)
}

/// The remembered placement of `workspace_name`, if any.
pub fn remembered_node(workspace_name: &str) -> Option<SocketAddr> {
    let path = placement_path()?;
    load_placement(&path)
        .workspaces
        .get(workspace_name)
        .copied()
}

/// Asks one node for its status.
pub async fn node_status(addr: SocketAddr) -> Result<StatusResponse> {
    let stream = tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(addr))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lists_and_dedups() {
        let nodes = parse_remotes("127.0.0.1:9400, 127.0.0.1:9401,127.0.0.1:9400").unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(parse_remotes(" , ").is_err());
    }

    #[test]
    fn rendezvous_is_stable_and_spreads() {
        let nodes = parse_remotes("10.0.0.1:9400,10.0.0.2:9400,10.0.0.3:9400").unwrap();
        let a = rendezvous_order(&nodes, "repo-a");
        assert_eq!(a, rendezvous_order(&nodes, "repo-a"));
        assert_eq!(a.len(), 3);
        // Removing a node keeps the relative order of the survivors.
        let fewer: Vec<SocketAddr> = nodes.iter().copied().filter(|n| *n != a[0]).collect();
        let b = rendezvous_order(&fewer, "repo-a");
        assert_eq!(
            b,
            a.into_iter()
                .filter(|n| fewer.contains(n))
                .collect::<Vec<_>>()
        );
        let homes: std::collections::HashSet<SocketAddr> = (0..64)
            .map(|i| rendezvous_order(&nodes, &format!("repo-{i}"))[0])
            .collect();
        assert_eq!(homes.len(), 3, "64 workspaces should land on all 3 nodes");
    }

    #[test]
    fn quietest_prefers_low_load_then_rendezvous_order() {
        let a: SocketAddr = "10.0.0.1:9400".parse().unwrap();
        let b: SocketAddr = "10.0.0.2:9400".parse().unwrap();
        let c: SocketAddr = "10.0.0.3:9400".parse().unwrap();
        assert_eq!(
            choose_quietest(&[(a, Some(0.8)), (b, Some(0.1)), (c, None)]),
            Some(b)
        );
        assert_eq!(choose_quietest(&[(a, None), (b, None)]), Some(a));
        assert_eq!(choose_quietest(&[(a, Some(0.2)), (b, Some(0.2))]), Some(a));
        assert_eq!(choose_quietest(&[]), None);
    }

    #[tokio::test]
    async fn picks_alive_node_and_remembers_it() {
        let temp = tempfile::tempdir().unwrap();
        let placement = temp.path().join("placement.json");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let alive = listener.local_addr().unwrap();
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let nodes = vec![dead, alive];
        let picked = pick_node_with(&nodes, "ws", Some(&placement))
            .await
            .unwrap();
        assert_eq!(picked, alive);
        let saved = load_placement(&placement);
        assert_eq!(saved.workspaces.get("ws"), Some(&alive));
        // A single node is used without probing.
        assert_eq!(pick_node_with(&[dead], "ws", None).await.unwrap(), dead);
        assert!(pick_node_with(&[dead], "", None).await.is_ok());
        assert!(pick_node_with(&[], "ws", None).await.is_err());
    }
}
