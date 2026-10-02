//! Multiple gateways (Phase 5.1, client side): a workspace is placed on one node by
//! rendezvous hashing over the reachable nodes, the placement is remembered locally so all
//! sessions of that checkout keep hitting the node whose engine and build cache are warm, and
//! a dead node fails over to the next one.

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ClusterResponse, MetricsRequest, MetricsResponse, PlaceRequest, PlaceResponse,
    ProdCodeCodec, StatusResponse, WireMessage, content_hash,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::codec::Framed;

/// How long a node has to accept a TCP connection before it counts as down.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// The cluster a process may route a request to by the path it names, and the workspace name
/// placements are remembered under. Set once at startup; unset in tests and in library use,
/// where every request goes to the node it was given.
struct Routing {
    nodes: Vec<SocketAddr>,
    workspace: String,
}

static ROUTING: std::sync::OnceLock<Routing> = std::sync::OnceLock::new();

/// Lets requests that name a path be sent to a node that serves that path's engine (#125).
pub fn set_routing(nodes: Vec<SocketAddr>, workspace: String) {
    let _ = ROUTING.set(Routing { nodes, workspace });
}

/// The node for a request about `path` in the checkout at `root`: `default` when the path
/// belongs to the checkout's own project, otherwise a node that serves the engine of the
/// nested project the path is in (a SwiftPM package or Xcode project under a Rust repository
/// needs the macOS node). That placement is remembered under its own key, `<workspace>#swift`,
/// so it does not displace the checkout's. Without routing set, or with one node, `default`.
pub async fn route_for_path(
    default: SocketAddr,
    root: &Path,
    path: Option<&str>,
) -> Result<SocketAddr> {
    let (Some(routing), Some(path)) = (ROUTING.get(), path) else {
        return Ok(default);
    };
    route_in(
        &routing.nodes,
        &routing.workspace,
        default,
        root,
        path,
        placement_path().as_deref(),
    )
    .await
}

/// The node for a query about another whole checkout at `root`: the one its own workspace is
/// placed on, picked as the CLI picks it when started there, so that checkout's warm analyzer
/// answers instead of a cold copy on this checkout's node (#375). Without routing set, or for
/// this checkout itself, `default`.
pub async fn route_for_checkout(default: SocketAddr, root: &Path) -> Result<SocketAddr> {
    let Some(routing) = ROUTING.get() else {
        return Ok(default);
    };
    checkout_node_in(
        &routing.nodes,
        &routing.workspace,
        default,
        root,
        placement_path().as_deref(),
    )
    .await
}

/// [`route_for_checkout`] over an explicit cluster, remembering placements in `placement_file`.
pub async fn checkout_node_in(
    nodes: &[SocketAddr],
    workspace: &str,
    default: SocketAddr,
    root: &Path,
    placement_file: Option<&Path>,
) -> Result<SocketAddr> {
    let identity = crate::sync::workspace_identity(root);
    let name = identity.base.unwrap_or(identity.name);
    if name == workspace {
        return Ok(default);
    }
    let (_, engine) = crate::sync::engine_project(root, root);
    let os = crate::sync::macos_only_cgo(root).map(|_| "macos");
    pick_node_with(nodes, &name, engine, os, placement_file).await
}

/// [`route_for_path`] over an explicit cluster, remembering placements in `placement_file`.
pub async fn route_in(
    nodes: &[SocketAddr],
    workspace: &str,
    default: SocketAddr,
    root: &Path,
    path: &str,
    placement_file: Option<&Path>,
) -> Result<SocketAddr> {
    let Some(engine) = nested_engine(root, path) else {
        return Ok(default);
    };
    if default_serves(default, engine).await {
        return Ok(default);
    }
    let key = format!("{workspace}#{engine}");
    pick_node_with(nodes, &key, Some(engine), None, placement_file)
        .await
        .with_context(|| format!("`{path}` is in a {engine} project"))
}

/// The engine of the nested project `path` is in, when that is not the checkout's own project:
/// `swift` for `swift/Sources/App/main.swift` under a Rust root. `None` for a path of the root
/// project.
pub fn nested_engine(root: &Path, path: &str) -> Option<&'static str> {
    let p = Path::new(path);
    let hint = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    match crate::sync::engine_project(root, &hint) {
        (Some(_), Some(engine)) => Some(engine),
        _ => None,
    }
}

/// Whether `node` lists `engine`.
async fn default_serves(node: SocketAddr, engine: &str) -> bool {
    node_status(node)
        .await
        .is_ok_and(|status| supports_engine(&status, engine))
}

/// Parses `host:port[,host:port...]` (spaces allowed) or `auto` into resolved addresses, in order.
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

/// Resolves cluster seed addresses automatically from cached cluster gossip, environment,
/// or loopback gateway (Roadmap 5.2).
pub fn resolve_auto_remotes() -> Result<Vec<SocketAddr>> {
    let mut nodes = Vec::new();
    // 1. Check cached cluster gossip file
    if let Some(path) = cluster_cache_path()
        && let Ok(bytes) = std::fs::read(&path)
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
    // 2. Check PROD_CODE_SEEDS environment variable if set
    if let Ok(seeds_spec) = std::env::var("PROD_CODE_SEEDS") {
        for s in seeds_spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if let Ok(addrs) = s.to_socket_addrs() {
                for addr in addrs {
                    if !nodes.contains(&addr) {
                        nodes.push(addr);
                    }
                }
            }
        }
    }
    // 3. Fallback to default loopback gateway address
    let loopback: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    if !nodes.contains(&loopback) {
        nodes.push(loopback);
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
        tokio::time::timeout(PROBE_TIMEOUT, prod_code_protocol::transport::connect(addr)).await,
        Ok(Ok(_))
    )
}

/// How many times an unreachable remembered node is asked again, and how long apart, before
/// the checkout moves: a gateway restarting for a deploy is back in about two seconds, and a
/// move leaves its warm analyzer behind (#238).
const RESTART_RETRIES: usize = 4;
const RESTART_WAIT: std::time::Duration = std::time::Duration::from_millis(750);

/// Can `node` take the workspace: alive, serving `engine` and running `os` when they are needed.
async fn node_fits(node: SocketAddr, engine: Option<&str>, os: Option<&str>) -> bool {
    if engine.is_none() && os.is_none() {
        return is_alive(node).await;
    }
    node_status(node)
        .await
        .is_ok_and(|status| status_fits(&status, engine, os))
}

/// Whether a gateway with `status` serves `engine` and runs `os`, those that are given.
fn status_fits(status: &StatusResponse, engine: Option<&str>, os: Option<&str>) -> bool {
    engine.is_none_or(|engine| supports_engine(status, engine))
        && os.is_none_or(|os| runs_os(status, os))
}

/// Whether a gateway runs `os` (`macos`, `linux`). One too old to report its platform does not.
pub fn runs_os(status: &StatusResponse, os: &str) -> bool {
    status
        .platform
        .as_deref()
        .is_some_and(|platform| platform.starts_with(os))
}

/// How an OS is written in a message: `macOS` for `macos`.
fn os_name(os: &str) -> &str {
    match os {
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
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

/// Remember the chosen node for `workspace_name` in the placement file.
pub fn remember_placement(workspace_name: &str, node: SocketAddr) {
    if let Some(path) = placement_path() {
        let mut placement = load_placement(&path);
        placement.workspaces.insert(workspace_name.to_string(), node);
        save_placement(&path, &placement);
    }
}

/// Whether a gateway lists `engine` (`rust`, `go`, `swift`, ...) among the engines it can
/// serve; entries look like `swift (sourcekit-lsp)`.
pub fn supports_engine(status: &StatusResponse, engine: &str) -> bool {
    status.detected_engines.iter().any(|e| {
        e == engine
            || e.strip_prefix(engine)
                .is_some_and(|rest| rest.starts_with(' '))
    })
}

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
                if still_fits {
                    if let Ok(status) = node_status(remembered).await {
                        if status.host.pressure().is_some() {
                            still_fits = false;
                        } else if status.congestion_score() >= 0.80 && nodes.len() > 1 {
                            // If remembered node has become congested, evaluate whether a significantly
                            // better node exists instead of blindly staying placed there.
                            if let Some((better, _)) =
                                evaluate_cluster_rebalance_with(nodes, remembered, workspace_name, engine, os).await
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
        placement.workspaces.insert(workspace_name.to_string(), *only);
        save_placement(path, &placement);
    }
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

/// Among candidates with their multi-dimensional congestion scores, chooses the best (lowest score) node;
/// ties keep rendezvous order.
pub fn choose_best_node(candidates: &[(SocketAddr, f64)]) -> Option<SocketAddr> {
    candidates
        .iter()
        .enumerate()
        .min_by(|(ia, (_, sa)), (ib, (_, sb))| {
            let key = |s: f64| (s * 1000.0) as i64;
            key(*sa).cmp(&key(*sb)).then(ia.cmp(ib))
        })
        .map(|(_, (addr, _))| *addr)
}

/// Evaluates whether the current remote node has degraded or become congested, and whether
/// a significantly more efficient, roomier node is available in the cluster.
/// Returns Some((new_addr, reason)) if rebalancing is recommended; None to remain on current node.
pub async fn evaluate_cluster_rebalance(
    current_node: SocketAddr,
    workspace_name: &str,
    engine: Option<&str>,
    os: Option<&str>,
) -> Option<(SocketAddr, String)> {
    if std::env::var_os("PROD_CODE_NO_REBALANCE").is_some() {
        return None;
    }

    // Single node / pinned check: if ROUTING is set and only lists current_node, do not rebalance.
    let cluster_nodes = if let Some(routing) = ROUTING.get() {
        if routing.nodes.len() <= 1 {
            return None;
        }
        routing.nodes.clone()
    } else {
        match resolve_auto_remotes() {
            Ok(nodes) if nodes.len() > 1 => nodes,
            _ => vec![current_node],
        }
    };

    evaluate_cluster_rebalance_with(&cluster_nodes, current_node, workspace_name, engine, os).await
}

/// [`evaluate_cluster_rebalance`] over an explicit list of cluster nodes.
pub async fn evaluate_cluster_rebalance_with(
    cluster_nodes: &[SocketAddr],
    current_node: SocketAddr,
    _workspace_name: &str,
    engine: Option<&str>,
    os: Option<&str>,
) -> Option<(SocketAddr, String)> {
    if std::env::var_os("PROD_CODE_NO_REBALANCE").is_some() || cluster_nodes.len() <= 1 {
        return None;
    }

    // Check status of current node
    let current_status = match node_status(current_node).await {
        Ok(s) => s,
        Err(_) => {
            // Current node is down: pick the best available node immediately
            let mut candidates = Vec::new();
            for node in cluster_nodes {
                if *node == current_node {
                    continue;
                }
                if let Ok(st) = node_status(*node).await {
                    if status_fits(&st, engine, os) && st.host.pressure().is_none() {
                        candidates.push((*node, st.congestion_score()));
                    }
                }
            }
            if let Some(best) = choose_best_node(&candidates) {
                return Some((
                    best,
                    format!("current node {current_node} is unreachable, migrating to live peer {best}"),
                ));
            }
            return None;
        }
    };

    let current_score = current_status.congestion_score();
    let has_pressure = current_status.host.pressure().is_some();

    // If current node is healthy and not congested (< 0.80 score and no pressure), keep warm caches
    if !has_pressure && current_score < 0.80 {
        return None;
    }

    // Try to get whole cluster state via cluster_view from current node or live peers
    let mut candidate_scores: Vec<(SocketAddr, f64, String)> = Vec::new();

    if let Ok(view) = cluster_view(current_node).await {
        for peer in view.nodes {
            if !peer.alive {
                continue;
            }
            let Ok(addr) = peer.addr.parse::<SocketAddr>() else {
                continue;
            };
            if addr == current_node {
                continue;
            }
            if !status_fits(&peer.status, engine, os) {
                continue;
            }
            if os.is_none() && runs_os(&peer.status, "macos") {
                // Don't migrate non-macOS work to macOS unless required
                continue;
            }
            let score = peer.status.congestion_score();
            candidate_scores.push((addr, score, peer.status.host.describe()));
        }
    } else {
        // Fallback: probe known nodes directly
        for node in cluster_nodes {
            if *node == current_node {
                continue;
            }
            if let Ok(st) = node_status(*node).await {
                if status_fits(&st, engine, os) {
                    if os.is_none() && runs_os(&st, "macos") {
                        continue;
                    }
                    candidate_scores.push((*node, st.congestion_score(), st.host.describe()));
                }
            }
        }
    }

    if candidate_scores.is_empty() {
        return None;
    }

    // Filter out candidates under hard pressure (>= 1000.0) if any non-pressured candidate exists
    let non_pressured: Vec<_> = candidate_scores
        .iter()
        .filter(|(_, s, _)| *s < 1000.0)
        .cloned()
        .collect();
    let pool = if !non_pressured.is_empty() {
        &non_pressured
    } else {
        &candidate_scores
    };

    // Find best candidate
    let best = pool
        .iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;

    let (best_addr, best_score, host_desc) = (best.0, best.1, &best.2);

    // Migration criteria with hysteresis:
    // 1. Current node is under hard pressure (score >= 1000.0) and best node is not
    if has_pressure && best_score < 1000.0 {
        return Some((
            best_addr,
            format!(
                "node {current_node} is under resource pressure ({}), migrating to roomier {best_addr} (score {:.2}, {host_desc})",
                current_status.host.pressure().unwrap_or_default(),
                best_score
            ),
        ));
    }

    // 2. Current node is noticeably congested (score >= 0.80), best node is at least 2x better and diff >= 0.40
    if current_score >= 0.80 && best_score < current_score * 0.50 && (current_score - best_score) >= 0.40 {
        return Some((
            best_addr,
            format!(
                "rebalancing from congested node {current_node} (score {:.2}) to quieter {best_addr} (score {:.2}, {host_desc})",
                current_score,
                best_score
            ),
        ));
    }

    None
}

/// The remembered placement of `workspace_name`, if any.
pub fn remembered_node(workspace_name: &str) -> Option<SocketAddr> {
    let path = placement_path()?;
    load_placement(&path)
        .workspaces
        .get(workspace_name)
        .copied()
}

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
    let stream = tokio::time::timeout(PROBE_TIMEOUT, prod_code_protocol::transport::connect(addr))
        .await
        .map_err(|_| anyhow!("connect timed out"))??;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::PlaceRequest(PlaceRequest {
            workspace_name: workspace_name.to_string(),
            engine: engine.map(String::from),
            os: os.map(String::from),
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

fn cluster_cache_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/prod_code/cluster.json"))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ClusterCache {
    #[serde(default)]
    nodes: Vec<String>,
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

    if learned
        && let Some(path) = placement_file
    {
        let mut placement = load_placement(path);
        let live_addrs: std::collections::HashSet<SocketAddr> = nodes.iter().copied().collect();
        let before = placement.workspaces.len();
        placement.workspaces.retain(|_, addr| live_addrs.contains(addr));
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

#[cfg(test)]
mod tests {
    use super::*;
    use prod_code_protocol::{HostResources, PeerInfo};

    #[test]
    fn parses_lists_and_dedups() {
        let nodes = parse_remotes("127.0.0.1:9400, 127.0.0.1:9401,127.0.0.1:9400").unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(parse_remotes(" , ").is_err());
    }

    #[test]
    fn parses_omitted_port_with_default() {
        let nodes = parse_remotes("127.0.0.1, localhost").unwrap();
        assert!(nodes.contains(&"127.0.0.1:9400".parse().unwrap()));
    }

    #[test]
    fn parses_auto_discovery_seeds() {
        let nodes = parse_remotes("auto").unwrap();
        assert!(!nodes.is_empty());
        assert!(nodes.contains(&"127.0.0.1:9400".parse().unwrap()));
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

    /// A path in a SwiftPM package under a Rust repository needs the swift engine; a path of
    /// the Rust project itself needs no other node (#125).
    #[tokio::test]
    async fn a_path_in_a_nested_project_names_its_engine() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"r\"\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::fs::create_dir_all(root.join("swift/Sources/App")).unwrap();
        std::fs::write(root.join("swift/Package.swift"), "").unwrap();
        std::fs::write(root.join("swift/Sources/App/main.swift"), "").unwrap();

        assert_eq!(nested_engine(&root, "swift"), Some("swift"));
        assert_eq!(
            nested_engine(&root, "swift/Sources/App/main.swift"),
            Some("swift")
        );
        let absolute = root.join("swift/Package.swift");
        assert_eq!(
            nested_engine(&root, &absolute.to_string_lossy()),
            Some("swift")
        );
        assert_eq!(nested_engine(&root, "src/lib.rs"), None);
        assert_eq!(nested_engine(&root, "."), None);

        // Without routing set up, a request goes where it was sent, whatever it names.
        let default: SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert_eq!(
            route_for_path(default, &root, Some("swift")).await.unwrap(),
            default
        );
    }

    /// A node that answers a status request with the engines it serves and nothing else.
    async fn node_serving(engines: &'static [&'static str]) -> SocketAddr {
        node_on(engines, Some("linux x86_64")).await
    }

    /// A node that answers a status request with the engines it serves and the platform it
    /// runs, `None` for a gateway too old to report one.
    async fn node_on(
        engines: &'static [&'static str],
        platform: Option<&'static str>,
    ) -> SocketAddr {
        node_with(engines, platform, 100, HostResources::default()).await
    }

    /// A node that answers a status request with the engines it serves, the platform it runs,
    /// its load average in thousandths over 4 CPUs, and what its host has left.
    async fn node_with(
        engines: &'static [&'static str],
        platform: Option<&'static str>,
        load_average_millis: u32,
        host: HostResources,
    ) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let host = host.clone();
                tokio::spawn(async move {
                    let mut framed = Framed::new(socket, ProdCodeCodec::new());
                    while let Some(Ok(message)) = framed.next().await {
                        if matches!(message, WireMessage::StatusRequest) {
                            let status = StatusResponse {
                                server_pid: 1,
                                uptime_seconds: 1,
                                active_sessions: 0,
                                loaded_workspaces: 0,
                                detected_engines: engines.iter().map(|e| e.to_string()).collect(),
                                memory_rss_bytes: None,
                                total_queries: 0,
                                active_queries: 0,
                                load_average_millis: Some(load_average_millis),
                                cpu_count: Some(4),
                                platform: platform.map(String::from),
                                running_commands: Vec::new(),
                                host: host.clone(),
                            };
                            let _ = framed.send(WireMessage::StatusResponse(status)).await;
                        } else {
                            return;
                        }
                    }
                });
            }
        });
        addr
    }

    /// Without a cluster answer, a node short of memory or disk is passed over for a busier one
    /// that has room, and taken only when every node is short (#396).
    #[tokio::test]
    async fn a_node_short_of_disk_is_passed_over_without_a_cluster_view() {
        let temp = tempfile::tempdir().unwrap();
        let placement = temp.path().join("placement.json");
        let full_disk = HostResources {
            storage_free_millis: Some(20),
            ..HostResources::default()
        };
        let roomy = HostResources {
            storage_free_millis: Some(600),
            ..HostResources::default()
        };
        let quiet_but_full =
            node_with(&["rust"], Some("linux x86_64"), 40, full_disk.clone()).await;
        let busier = node_with(&["rust"], Some("linux x86_64"), 2000, roomy).await;
        let chosen = pick_node_with(
            &[quiet_but_full, busier],
            "subject",
            Some("rust"),
            None,
            Some(&placement),
        )
        .await
        .unwrap();
        assert_eq!(chosen, busier);

        let also_full = node_with(&["rust"], Some("linux x86_64"), 2000, full_disk).await;
        let chosen = pick_node_with(
            &[quiet_but_full, also_full],
            "other",
            Some("rust"),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(chosen, quiet_but_full, "every node is short: the quietest");
    }

    /// With a Rust node and a Swift node, a path in the SwiftPM package goes to the Swift node and
    /// is remembered under its own key; a Rust path stays on the Rust node; with no Swift node the
    /// error says what the path needs (#125).
    #[tokio::test]
    async fn a_swift_path_is_routed_to_the_node_that_serves_swift() {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"r\"\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::fs::create_dir_all(root.join("swift/Sources/App")).unwrap();
        std::fs::write(root.join("swift/Package.swift"), "").unwrap();
        let placement = temp.path().join("placement.json");

        let rust = node_serving(&["rust", "go"]).await;
        let swift = node_serving(&["swift (sourcekit-lsp)"]).await;
        let nodes = [rust, swift];
        let routed = route_in(&nodes, "mixed", rust, &root, "swift", Some(&placement))
            .await
            .unwrap();
        assert_eq!(routed, swift);
        assert_eq!(
            load_placement(&placement).workspaces.get("mixed#swift"),
            Some(&swift)
        );
        assert!(!load_placement(&placement).workspaces.contains_key("mixed"));
        let stays = route_in(&nodes, "mixed", rust, &root, "src/lib.rs", Some(&placement))
            .await
            .unwrap();
        assert_eq!(stays, rust);
        // A default node that serves the engine keeps the call.
        let kept = route_in(&nodes, "mixed", swift, &root, "swift", Some(&placement))
            .await
            .unwrap();
        assert_eq!(kept, swift);

        let other_rust = node_serving(&["rust"]).await;
        let err = route_in(&[rust, other_rust], "mixed", rust, &root, "swift", None)
            .await
            .expect_err("no node serves swift");
        let text = format!("{err:#}");
        assert!(text.contains("is in a swift project"), "{text}");
        assert!(text.contains("serves swift"), "{text}");
    }

    /// Another checkout is asked on the node its own workspace is placed on, not on this one's;
    /// this checkout itself stays on the node it was given (#375).
    #[tokio::test]
    async fn another_checkout_is_routed_to_its_own_node() {
        let temp = tempfile::tempdir().unwrap();
        let other = std::fs::canonicalize(temp.path()).unwrap().join("other");
        std::fs::create_dir_all(other.join("src")).unwrap();
        std::fs::write(other.join("Cargo.toml"), "[package]\nname = \"o\"\n").unwrap();
        std::fs::write(other.join("src/lib.rs"), "").unwrap();
        let identity = crate::sync::workspace_identity(&other);
        let name = identity.base.unwrap_or(identity.name);
        let placement = temp.path().join("placement.json");
        let here = node_serving(&["rust"]).await;
        let there = node_serving(&["rust"]).await;
        let mut remembered = Placement::default();
        remembered.workspaces.insert(name.clone(), there);
        save_placement(&placement, &remembered);

        let nodes = [here, there];
        let routed = checkout_node_in(&nodes, "this", here, &other, Some(&placement))
            .await
            .unwrap();
        assert_eq!(routed, there);
        let itself = checkout_node_in(&nodes, &name, here, &other, Some(&placement))
            .await
            .unwrap();
        assert_eq!(itself, here);
    }

    /// A checkout that needs macOS leaves a remembered Linux node for the macOS one, and a node
    /// too old to report its platform does not count as macOS. Without a macOS node the error
    /// says so, with one node or several; a checkout that needs no OS still takes Linux (#248).
    #[tokio::test]
    async fn a_checkout_that_needs_macos_is_placed_on_a_macos_node() {
        let temp = tempfile::tempdir().unwrap();
        let placement = temp.path().join("placement.json");
        let linux = node_on(&["go"], Some("linux x86_64")).await;
        let old = node_on(&["go"], None).await;
        let mac = node_on(&["go", "swift (sourcekit-lsp)"], Some("macos aarch64")).await;
        let mut remembered = Placement::default();
        remembered.workspaces.insert("cgo".to_string(), linux);
        save_placement(&placement, &remembered);

        let nodes = [linux, old, mac];
        let picked = pick_node_with(&nodes, "cgo", Some("go"), Some("macos"), Some(&placement))
            .await
            .unwrap();
        assert_eq!(picked, mac);
        assert_eq!(load_placement(&placement).workspaces.get("cgo"), Some(&mac));
        let anywhere = pick_node_with(&[linux, old], "plain", Some("go"), None, None)
            .await
            .unwrap();
        assert!(anywhere == linux || anywhere == old, "{anywhere}");

        let err = pick_node_with(&[linux, old], "cgo", Some("go"), Some("macos"), None)
            .await
            .expect_err("no node runs macOS");
        let text = format!("{err:#}");
        assert!(text.contains("no reachable gateway runs macOS"), "{text}");
        assert!(text.contains(&linux.to_string()), "{text}");
        assert!(text.contains(&old.to_string()), "{text}");
        let err = pick_node_with(&[linux], "cgo", Some("go"), Some("macos"), None)
            .await
            .expect_err("the one node runs Linux");
        assert!(
            format!("{err:#}").contains("no reachable gateway runs macOS"),
            "{err:#}"
        );
        assert_eq!(
            pick_node_with(&[mac], "cgo", Some("go"), Some("macos"), None)
                .await
                .unwrap(),
            mac
        );
    }

    /// Without a cluster answer, work that does not need macOS still keeps off a macOS node
    /// while another node serves it, whatever the rendezvous order says, and takes the Mac only
    /// when nothing else can (#308).
    #[tokio::test]
    async fn plain_work_keeps_off_a_macos_node_while_another_node_serves_it() {
        let linux = node_on(&["go"], Some("linux x86_64")).await;
        let mac = node_on(&["go", "swift (sourcekit-lsp)"], Some("macos aarch64")).await;
        for i in 0..20 {
            let name = format!("plain-{i}");
            let picked = pick_node_with(&[mac, linux], &name, Some("go"), None, None)
                .await
                .unwrap();
            assert_eq!(picked, linux, "{name} went to the Mac");
        }
        assert_eq!(
            pick_node_with(&[mac, linux], "app", Some("swift"), None, None)
                .await
                .unwrap(),
            mac,
            "only the Mac serves Swift"
        );
        assert_eq!(
            pick_node_with(&[mac], "plain", Some("go"), None, None)
                .await
                .unwrap(),
            mac,
            "alone, the Mac takes plain Go"
        );
    }

    #[test]
    fn engine_support_matches_labelled_entries() {
        let status = StatusResponse {
            server_pid: 1,
            uptime_seconds: 0,
            active_sessions: 0,
            loaded_workspaces: 0,
            detected_engines: vec![
                "rust (ra_ap_ide)".to_string(),
                "swift (sourcekit-lsp)".to_string(),
                "generic-lsp".to_string(),
            ],
            memory_rss_bytes: None,
            total_queries: 0,
            active_queries: 0,
            load_average_millis: None,
            cpu_count: None,
            platform: None,
            running_commands: Vec::new(),
            host: Default::default(),
        };
        assert!(supports_engine(&status, "rust"));
        assert!(supports_engine(&status, "swift"));
        assert!(supports_engine(&status, "generic-lsp"));
        assert!(!supports_engine(&status, "go"));
        assert!(!supports_engine(&status, "swif"));
    }

    /// A remembered node that is down for a moment, as a gateway is while it restarts, keeps the
    /// workspace; one that stays down loses it to a live node (#238).
    #[tokio::test]
    async fn a_remembered_node_that_restarts_keeps_the_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let placement = temp.path().join("placement.json");
        let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let other_addr = other.local_addr().unwrap();
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let home = closed.local_addr().unwrap();
        drop(closed);
        let mut remembered = Placement::default();
        remembered.workspaces.insert("ws".to_string(), home);
        save_placement(&placement, &remembered);
        let revived = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;
            let listener = tokio::net::TcpListener::bind(home).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            drop(listener);
        });
        let picked = pick_node_with(&[other_addr, home], "ws", None, None, Some(&placement))
            .await
            .unwrap();
        assert_eq!(picked, home, "the restarted node keeps the workspace");
        revived.abort();

        // Down for good: after the retries the workspace moves to the live node.
        let gone = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gone_addr = gone.local_addr().unwrap();
        drop(gone);
        let mut remembered = Placement::default();
        remembered.workspaces.insert("ws2".to_string(), gone_addr);
        save_placement(&placement, &remembered);
        let picked = pick_node_with(
            &[other_addr, gone_addr],
            "ws2",
            None,
            None,
            Some(&placement),
        )
        .await
        .unwrap();
        assert_eq!(picked, other_addr);
        drop(other);
    }

    #[tokio::test]
    async fn picks_alive_node_and_remembers_it() {
        let temp = tempfile::tempdir().unwrap();
        let placement = temp.path().join("placement.json");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let alive = listener.local_addr().unwrap();
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let nodes = vec![dead, alive];
        let picked = pick_node_with(&nodes, "ws", None, None, Some(&placement))
            .await
            .unwrap();
        assert_eq!(picked, alive);
        let saved = load_placement(&placement);
        assert_eq!(saved.workspaces.get("ws"), Some(&alive));
        // A single node is used without probing.
        assert_eq!(
            pick_node_with(&[dead], "ws", None, None, None)
                .await
                .unwrap(),
            dead
        );
        assert!(pick_node_with(&[dead], "", None, None, None).await.is_ok());
        assert!(pick_node_with(&[], "ws", None, None, None).await.is_err());
    }

    /// When no node answers, the error says why the first did not (#340).
    #[tokio::test]
    async fn no_node_reachable_names_why_the_first_did_not_answer() {
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let also_dead: SocketAddr = "127.0.0.1:2".parse().unwrap();
        let err = pick_node_with(&[dead, also_dead], "ws", Some("rust"), None, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("no gateway reachable among"), "{err}");
        assert!(
            err.contains("127.0.0.1:") && err.to_lowercase().contains("refused"),
            "the operating system's reason is named: {err}"
        );
        assert!(err.contains("ssh -NL 9400:localhost:9400"), "{err}");
    }

    #[tokio::test]
    async fn retains_remembered_placement_when_single_seed_unsupported_or_down() {
        let temp = tempfile::tempdir().unwrap();
        let placement = temp.path().join("placement.json");
        let mac = node_on(&["swift (sourcekit-lsp)"], Some("macos aarch64")).await;
        let mut remembered = Placement::default();
        remembered.workspaces.insert("ws#swift".to_string(), mac);
        save_placement(&placement, &remembered);

        let dead_loopback: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let picked = pick_node_with(
            &[dead_loopback],
            "ws#swift",
            Some("swift"),
            None,
            Some(&placement),
        )
        .await
        .unwrap();
        assert_eq!(picked, mac, "retains remembered mac node for swift workspace");
    }

    async fn seed_node_with_peers(peers: Vec<SocketAddr>) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let peers = peers.clone();
                tokio::spawn(async move {
                    let mut framed = Framed::new(socket, ProdCodeCodec::new());
                    while let Some(Ok(message)) = framed.next().await {
                        match message {
                            WireMessage::ClusterRequest => {
                                let resp = ClusterResponse {
                                    this_node: addr.to_string(),
                                    nodes: peers
                                        .iter()
                                        .map(|p| PeerInfo {
                                            addr: p.to_string(),
                                            status: StatusResponse {
                                                server_pid: 1,
                                                uptime_seconds: 1,
                                                active_sessions: 0,
                                                loaded_workspaces: 0,
                                                detected_engines: vec!["rust".to_string()],
                                                memory_rss_bytes: None,
                                                total_queries: 0,
                                                active_queries: 0,
                                                load_average_millis: Some(100),
                                                cpu_count: Some(4),
                                                platform: Some("linux x86_64".to_string()),
                                                running_commands: Vec::new(),
                                                host: HostResources::default(),
                                            },
                                            last_seen_secs: 0,
                                            workspaces: vec![],
                                            alive: true,
                                        })
                                        .collect(),
                                };
                                let _ = framed.send(WireMessage::ClusterResponse(resp)).await;
                            }
                            WireMessage::StatusRequest => {
                                let status = StatusResponse {
                                    server_pid: 1,
                                    uptime_seconds: 1,
                                    active_sessions: 0,
                                    loaded_workspaces: 0,
                                    detected_engines: vec!["rust".to_string()],
                                    memory_rss_bytes: None,
                                    total_queries: 0,
                                    active_queries: 0,
                                    load_average_millis: Some(100),
                                    cpu_count: Some(4),
                                    platform: Some("linux x86_64".to_string()),
                                    running_commands: Vec::new(),
                                    host: HostResources::default(),
                                };
                                let _ = framed.send(WireMessage::StatusResponse(status)).await;
                            }
                            _ => return,
                        }
                    }
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn discover_nodes_preserves_remembered_placement_when_seed_view_omits_it() {
        let temp = tempfile::tempdir().unwrap();
        let placement = temp.path().join("placement.json");
        let cache = temp.path().join("cluster.json");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let seed_addr = listener.local_addr().unwrap();
        drop(listener);
        let seed = seed_node_with_peers(vec![seed_addr]).await;

        let mac = node_on(&["swift (sourcekit-lsp)"], Some("macos aarch64")).await;
        let mut rem = Placement::default();
        rem.workspaces.insert("ws#swift".to_string(), mac);
        save_placement(&placement, &rem);

        let discovered = discover_nodes_with_paths(&[seed], Some(&placement), Some(&cache)).await;

        assert!(
            discovered.contains(&mac),
            "discovered nodes must include remembered placement {mac}: {discovered:?}"
        );
        assert!(
            discovered.contains(&seed),
            "discovered nodes must include seed {seed}: {discovered:?}"
        );

        let picked = pick_node_with(
            &discovered,
            "ws#swift",
            Some("swift"),
            None,
            Some(&placement),
        )
        .await
        .unwrap();
        assert_eq!(picked, mac, "pick_node_with must choose remembered mac node");
    }

    #[tokio::test]
    async fn explicit_remote_does_not_fall_back_to_remembered_placement() {
        let temp = tempfile::tempdir().unwrap();
        let placement = temp.path().join("placement.json");

        let remembered = node_on(&["rust"], Some("linux x86_64")).await;
        let mut rem = Placement::default();
        rem.workspaces.insert("subject".to_string(), remembered);
        save_placement(&placement, &rem);

        let explicit_remote: SocketAddr = "192.168.2.168:9400".parse().unwrap();
        let picked = pick_node_with(
            &[explicit_remote],
            "subject",
            Some("rust"),
            None,
            Some(&placement),
        )
        .await
        .unwrap();

        assert_eq!(picked, explicit_remote);
    }

    #[tokio::test]
    async fn default_loopback_remote_falls_back_to_remembered_placement_when_dead() {
        let temp = tempfile::tempdir().unwrap();
        let placement = temp.path().join("placement.json");

        let remembered = node_on(&["rust"], Some("linux x86_64")).await;
        let mut rem = Placement::default();
        rem.workspaces.insert("subject".to_string(), remembered);
        save_placement(&placement, &rem);

        let default_loopback: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        if !is_alive(default_loopback).await {
            let picked = pick_node_with(
                &[default_loopback],
                "subject",
                Some("rust"),
                None,
                Some(&placement),
            )
            .await
            .unwrap();
            assert_eq!(picked, remembered);
        }
    }

    #[test]
    fn choose_best_node_picks_lowest_congestion_score() {
        let node1: SocketAddr = "10.0.0.1:9400".parse().unwrap();
        let node2: SocketAddr = "10.0.0.2:9400".parse().unwrap();
        let node3: SocketAddr = "10.0.0.3:9400".parse().unwrap();

        let candidates = vec![
            (node1, 1.45),
            (node2, 0.05),
            (node3, 0.82),
        ];

        assert_eq!(choose_best_node(&candidates), Some(node2));
    }

    #[tokio::test]
    async fn pick_node_with_rebalances_when_remembered_node_is_congested() {
        let temp = tempfile::tempdir().unwrap();
        let placement = temp.path().join("placement.json");

        let roomy_disk = HostResources {
            storage_free_millis: Some(600),
            ..HostResources::default()
        };

        // Congested node: 4.8 load / 4 cpus = 1.2 load/cpu, score ~1.2
        let congested = node_with(&["rust"], Some("linux x86_64"), 4800, roomy_disk.clone()).await;
        // Roomy node: 0.08 load / 4 cpus = 0.02 load/cpu, score ~0.02
        let roomy = node_with(&["rust"], Some("linux x86_64"), 80, roomy_disk).await;

        let mut rem = Placement::default();
        rem.workspaces.insert("subject".to_string(), congested);
        save_placement(&placement, &rem);

        // Before picking, placement points to `congested`
        assert_eq!(load_placement(&placement).workspaces.get("subject"), Some(&congested));

        // When picking, it detects that `congested` has score >= 0.80 and `roomy` is >2x better,
        // so it rebalances to `roomy` and updates placement.json!
        let chosen = pick_node_with(
            &[congested, roomy],
            "subject",
            Some("rust"),
            None,
            Some(&placement),
        )
        .await
        .unwrap();

        assert_eq!(chosen, roomy);
        assert_eq!(load_placement(&placement).workspaces.get("subject"), Some(&roomy));
    }
}

