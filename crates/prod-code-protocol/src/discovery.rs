//! Lightweight UDP discovery for the prod-code cluster.
//!
//! # Protocol
//!
//! One UDP port (`DISCOVERY_PORT`, 9401) carries both multicast and unicast messages.
//!
//! **Probe** (client → gateway, multicast or unicast):
//! ```text
//! PROD_CODE_DISCOVER\n
//! ```
//!
//! **Announce** (gateway → multicast, or gateway → client as probe reply):
//! ```text
//! PROD_CODE_NODE <advertise_addr> <engines_csv> <rss_mb> <load_per_cpu>\n
//! ```
//!
//! A gateway that receives a probe replies with its own announce **plus** one line per
//! known peer, so a single unicast probe to any reachable node returns the full cluster.
//!
//! # Multicast
//!
//! Gateways join `MULTICAST_GROUP` (`239.255.80.67`) on startup and send their announce
//! every `ANNOUNCE_PERIOD`.  Clients (and other gateways) listen on the same group to
//! discover nodes without any configuration.
//!
//! # Unicast fallback
//!
//! When multicast is unavailable (different subnet, firewall), a client sends the probe
//! as a unicast UDP packet to a known seed address on `DISCOVERY_PORT`.  The seed replies
//! with the full cluster view.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

/// The UDP port used for discovery (one above the gateway TCP port).
pub const DISCOVERY_PORT: u16 = 9401;

/// Multicast group for LAN announcements.
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 80, 67);

/// How often a gateway announces itself on multicast.
pub const ANNOUNCE_PERIOD: Duration = Duration::from_secs(5);

/// How long a client collects multicast/unicast replies before returning.
pub const COLLECT_TIMEOUT: Duration = Duration::from_millis(250);

/// Maximum UDP datagram we handle.
const MAX_DGRAM: usize = 4096;

const PROBE_LINE: &[u8] = b"PROD_CODE_DISCOVER\n";
const NODE_PREFIX: &str = "PROD_CODE_NODE ";

// ── Data types ────────────────────────────────────────────────────────────────

/// A workspace loaded on a discovered node.
#[derive(Debug, Clone)]
pub struct LoadedWorkspace {
    pub name: String,
    pub engine: String,
    pub sessions: u32,
}

/// A discovered node with full routing metadata.
#[derive(Debug, Clone)]
pub struct DiscoveredNode {
    pub addr: SocketAddr,
    pub engines: Vec<String>,
    pub rss_mb: u64,
    pub load_per_cpu: f64,
    /// Number of logical CPUs.
    pub cpus: u32,
    /// Total host RAM in MB.
    pub mem_total_mb: u64,
    /// Available (free) host RAM in MB.
    pub mem_avail_mb: u64,
    /// Active client sessions.
    pub sessions: u32,
    /// Workspaces currently loaded in memory (name:engine:sessions).
    pub workspaces: Vec<LoadedWorkspace>,
}

/// Format: `PROD_CODE_NODE <addr> <engines> <rss_mb> <load> <cpus> <mem_total> <mem_avail> <sessions> [ws1:eng:n,ws2:eng:n,...]`
///
/// Workspaces are comma-separated `name:engine:sessions` triples.  A `-` means none loaded.
fn parse_node_line(line: &str) -> Option<DiscoveredNode> {
    let rest = line.strip_prefix(NODE_PREFIX)?;
    let mut parts = rest.split_whitespace();
    let addr: SocketAddr = parts.next()?.parse().ok()?;
    let engines: Vec<String> = parts
        .next()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let rss_mb: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let load_per_cpu: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let cpus: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mem_total_mb: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mem_avail_mb: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let sessions: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let workspaces = parts
        .next()
        .filter(|s| *s != "-")
        .map(|ws_csv| {
            ws_csv
                .split(',')
                .filter_map(|entry| {
                    let mut p = entry.splitn(3, ':');
                    let name = p.next()?;
                    let engine = p.next().unwrap_or("?");
                    let sess: u32 = p.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    Some(LoadedWorkspace {
                        name: name.to_string(),
                        engine: engine.to_string(),
                        sessions: sess,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(DiscoveredNode {
        addr,
        engines,
        rss_mb,
        load_per_cpu,
        cpus,
        mem_total_mb,
        mem_avail_mb,
        sessions,
        workspaces,
    })
}

/// Format one announce line (without trailing newline).
pub fn format_node_line(
    advertise: &str,
    engines_csv: &str,
    rss_mb: u64,
    load_per_cpu: f64,
    cpus: u32,
    mem_total_mb: u64,
    mem_avail_mb: u64,
    sessions: u32,
    workspaces: &[(String, String, u32)], // (name, engine, sessions)
) -> String {
    let ws = if workspaces.is_empty() {
        "-".to_string()
    } else {
        workspaces
            .iter()
            .map(|(n, e, s)| format!("{n}:{e}:{s}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    format!(
        "{NODE_PREFIX}{advertise} {engines_csv} {rss_mb} {load_per_cpu:.4} {cpus} {mem_total_mb} {mem_avail_mb} {sessions} {ws}"
    )
}

// ── Gateway side ──────────────────────────────────────────────────────────────

/// Bind the discovery UDP socket.  The gateway calls this once at startup.
///
/// Returns the socket (non-blocking) and a `JoinHandle` is **not** returned — the
/// caller drives the socket with [`handle_incoming`] and [`send_announce`].
pub fn bind_discovery_socket() -> std::io::Result<UdpSocket> {
    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    sock.set_reuse_address(true)?;
    // macOS needs SO_REUSEPORT for multiple processes on the same host.
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    sock.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT).into())?;
    sock.join_multicast_v4(&MULTICAST_GROUP, &Ipv4Addr::UNSPECIFIED)?;
    sock.set_multicast_loop_v4(false)?;
    // Non-blocking so tokio can poll it.
    sock.set_nonblocking(true)?;
    Ok(sock.into())
}

/// Process one incoming datagram.  If it is a probe, returns the source address
/// so the caller can reply.
pub fn handle_incoming(buf: &[u8], _from: SocketAddr) -> Option<SocketAddr> {
    if buf.starts_with(b"PROD_CODE_DISCOVER") {
        Some(_from)
    } else if let Ok(text) = std::str::from_utf8(buf) {
        // Announce from another gateway — caller absorbs it.
        for line in text.lines() {
            if let Some(_node) = parse_node_line(line) {
                // Caller will collect these.
            }
        }
        None
    } else {
        None
    }
}

/// Build the reply payload: own node + known peers, one line each.
pub fn build_reply(own_line: &str, peer_lines: &[String]) -> Vec<u8> {
    let mut payload = String::with_capacity(256);
    payload.push_str(own_line);
    payload.push('\n');
    for peer in peer_lines {
        payload.push_str(peer);
        payload.push('\n');
    }
    payload.into_bytes()
}

/// Send the multicast announce.
pub fn send_announce(sock: &UdpSocket, payload: &[u8]) -> std::io::Result<()> {
    let dest = SocketAddrV4::new(MULTICAST_GROUP, DISCOVERY_PORT);
    sock.send_to(payload, dest)?;
    Ok(())
}

// ── Client side ───────────────────────────────────────────────────────────────

/// Discover nodes by sending a probe and collecting replies.
///
/// 1. If `seeds` is empty, sends a **multicast** probe and listens.
/// 2. If `seeds` is non-empty, sends a **unicast** probe to each seed, then also
///    sends a multicast probe.  Collects all replies for `COLLECT_TIMEOUT`.
///
/// Returns the de-duplicated list of discovered nodes.
pub fn discover(seeds: &[SocketAddr]) -> Vec<DiscoveredNode> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(COLLECT_TIMEOUT));

    // Send probes.
    let mcast_dest = SocketAddrV4::new(MULTICAST_GROUP, DISCOVERY_PORT);
    let _ = sock.send_to(PROBE_LINE, mcast_dest);
    for seed in seeds {
        let unicast_dest = SocketAddr::new(seed.ip(), DISCOVERY_PORT);
        let _ = sock.send_to(PROBE_LINE, unicast_dest);
    }

    // Collect replies.
    let mut nodes: HashMap<SocketAddr, DiscoveredNode> = HashMap::new();
    let deadline = Instant::now() + COLLECT_TIMEOUT;
    let mut buf = [0u8; MAX_DGRAM];
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let _ = sock.set_read_timeout(Some(remaining.max(Duration::from_millis(1))));
        match sock.recv_from(&mut buf) {
            Ok((n, _from)) => {
                if let Ok(text) = std::str::from_utf8(&buf[..n]) {
                    for line in text.lines() {
                        if let Some(node) = parse_node_line(line) {
                            nodes.entry(node.addr).or_insert(node);
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }
    nodes.into_values().collect()
}

/// Convenience: discover and return just the `SocketAddr`s.
pub fn discover_addrs(seeds: &[SocketAddr]) -> Vec<SocketAddr> {
    discover(seeds).into_iter().map(|n| n.addr).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_parse_round_trips() {
        let ws = vec![
            ("prod-code".into(), "rust".into(), 2),
            ("CodeHaus".into(), "go".into(), 1),
        ];
        let line = format_node_line(
            "192.168.2.168:9400", "rust,go,python", 4200, 0.0712,
            32, 128000, 64000, 3, &ws,
        );
        let node = parse_node_line(&line).expect("should parse");
        assert_eq!(node.addr, "192.168.2.168:9400".parse().unwrap());
        assert_eq!(node.engines, vec!["rust", "go", "python"]);
        assert_eq!(node.rss_mb, 4200);
        assert!((node.load_per_cpu - 0.0712).abs() < 0.001);
        assert_eq!(node.cpus, 32);
        assert_eq!(node.mem_total_mb, 128000);
        assert_eq!(node.mem_avail_mb, 64000);
        assert_eq!(node.sessions, 3);
        assert_eq!(node.workspaces.len(), 2);
        assert_eq!(node.workspaces[0].name, "prod-code");
        assert_eq!(node.workspaces[0].engine, "rust");
        assert_eq!(node.workspaces[0].sessions, 2);
        assert_eq!(node.workspaces[1].name, "CodeHaus");
    }

    #[test]
    fn parse_minimal_line() {
        // Old-format line with only 4 fields: backward compatible, new fields default.
        let line = "PROD_CODE_NODE 10.0.0.1:9400 rust 0 0.0";
        let node = parse_node_line(line).unwrap();
        assert_eq!(node.addr, "10.0.0.1:9400".parse().unwrap());
        assert_eq!(node.engines, vec!["rust"]);
        assert_eq!(node.cpus, 0);
        assert_eq!(node.mem_total_mb, 0);
        assert!(node.workspaces.is_empty());
    }

    #[test]
    fn parse_garbage_returns_none() {
        assert!(parse_node_line("hello world").is_none());
        assert!(parse_node_line("PROD_CODE_NODE badaddr rust 0 0").is_none());
    }

    #[test]
    fn no_workspaces_shows_dash() {
        let line = format_node_line("10.0.0.1:9400", "rust", 100, 0.05, 4, 8000, 4000, 0, &[]);
        assert!(line.ends_with(" -"));
        let node = parse_node_line(&line).unwrap();
        assert!(node.workspaces.is_empty());
    }

    #[test]
    fn build_reply_contains_all_lines() {
        let own = format_node_line(
            "10.0.0.1:9400", "rust", 100, 0.05, 4, 8000, 4000, 0, &[],
        );
        let peers = vec![format_node_line(
            "10.0.0.2:9400", "go", 200, 0.10, 8, 16000, 8000, 1,
            &[("myapp".into(), "go".into(), 1)],
        )];
        let payload = build_reply(&own, &peers);
        let text = String::from_utf8(payload).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("10.0.0.1:9400"));
        assert!(lines[1].contains("10.0.0.2:9400"));
        assert!(lines[1].contains("myapp:go:1"));
    }
}
