//! Lightweight UDP discovery for the prod-code cluster.
//!
//! # Protocol
//!
//! One UDP port (`DISCOVERY_PORT`, 9401) carries both multicast and unicast messages.
//!
//! **Probe** (client → gateway, multicast or unicast):
//! ```text
//! PROD_CODE_DISCOVER [<auth_tag>]\n
//! ```
//!
//! **Announce** (gateway → multicast, or gateway → client as probe reply):
//! ```text
//! PROD_CODE_NODE <auth_tag> <advertise_addr> <engines_csv> <rss_mb> <load> <cpus> <mem_total> <mem_avail> <sessions> [ws1:eng:n,ws2:eng:n,...]\n
//! ```
//!
//! When an auth token is configured, announcements and probes include a cryptographic
//! MAC tag derived from the token to prevent rogue LAN endpoints from poisoning
//! cluster selection, capturing tokens, or harvesting telemetry.
//!
//! In addition, the client validates that the advertised IP address matches the
//! sender's IP address (`recv_from`), preventing cross-host spoofing attacks on LAN.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
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

const PROBE_PREFIX: &str = "PROD_CODE_DISCOVER";
const NODE_PREFIX: &str = "PROD_CODE_NODE ";

// ── Auth MAC & Escaping Helpers ───────────────────────────────────────────────

/// Computes a 16-hex-character MAC tag using the cluster auth token.
pub fn compute_auth_tag(token: &str, data: &str) -> String {
    let combined = format!("{token}#{data}");
    format!("{:016x}", crate::content_hash(combined.as_bytes()))
}

/// Verifies an auth tag against the expected token.
pub fn verify_auth_tag(token: &str, data: &str, tag: &str) -> bool {
    let expected = compute_auth_tag(token, data);
    expected == tag
}

/// Percent-encode arbitrary workspace names so spaces, commas, and colons
/// do not corrupt the whitespace- and delimiter-separated UDP record.
pub fn encode_workspace_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.as_bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(*b as char),
            other => {
                use std::fmt::Write;
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

/// Decode percent-encoded workspace name back to UTF-8.
pub fn decode_workspace_name(encoded: &str) -> String {
    let mut bytes = Vec::new();
    let mut chars = encoded.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next().unwrap_or('0');
            let h2 = chars.next().unwrap_or('0');
            if let Ok(b) = u8::from_str_radix(&format!("{h1}{h2}"), 16) {
                bytes.push(b);
            }
        } else {
            bytes.push(c as u8);
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| encoded.to_string())
}

// ── Data types ────────────────────────────────────────────────────────────────

/// A workspace loaded on a discovered node.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Parse one `PROD_CODE_NODE` line, validating anti-spoofing and auth token MAC.
pub fn parse_node_line_with_auth(
    line: &str,
    expected_token: Option<&str>,
    sender_ip: Option<IpAddr>,
) -> Option<DiscoveredNode> {
    let rest = line.strip_prefix(NODE_PREFIX)?;
    let mut parts = rest.split_whitespace();

    let first = parts.next()?;
    // Support backward-compatible lines where field 0 is addr, or new lines where field 0 is tag.
    let (tag, addr_str) = if let Ok(_) = first.parse::<SocketAddr>() {
        ("-", first)
    } else {
        (first, parts.next()?)
    };

    let addr: SocketAddr = addr_str.parse().ok()?;

    // Anti-spoofing check: advertised IP must match actual UDP packet sender IP
    // (unless one is loopback, as in local tests).
    if let Some(from_ip) = sender_ip {
        if !from_ip.is_loopback() && !addr.ip().is_loopback() && from_ip != addr.ip() {
            tracing::warn!(from = %from_ip, advertised = %addr, "rejecting spoofed discovery advertisement");
            return None;
        }
    }

    let engines_csv = parts.next().unwrap_or("");
    let engines: Vec<String> = engines_csv
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

    // Auth verification: if an auth token is configured, reject announcements with invalid MAC tag.
    if let Some(token) = expected_token {
        let auth_payload = format!("{addr}:{engines_csv}:{cpus}:{mem_total_mb}");
        if !verify_auth_tag(token, &auth_payload, tag) {
            tracing::warn!(addr = %addr, "rejecting unauthenticated/forged discovery advertisement");
            return None;
        }
    }

    let workspaces = parts
        .next()
        .filter(|s| *s != "-")
        .map(|ws_csv| {
            ws_csv
                .split(',')
                .filter_map(|entry| {
                    let mut p = entry.splitn(3, ':');
                    let raw_name = p.next()?;
                    let engine = p.next().unwrap_or("?");
                    let sess: u32 = p.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    Some(LoadedWorkspace {
                        name: decode_workspace_name(raw_name),
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

/// Convenience parser without anti-spoofing or auth verification (used by unit tests).
pub fn parse_node_line(line: &str) -> Option<DiscoveredNode> {
    parse_node_line_with_auth(line, None, None)
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
    token: Option<&str>,
) -> String {
    let ws = if workspaces.is_empty() {
        "-".to_string()
    } else {
        workspaces
            .iter()
            .map(|(n, e, s)| {
                let enc_name = encode_workspace_name(n);
                format!("{enc_name}:{e}:{s}")
            })
            .collect::<Vec<_>>()
            .join(",")
    };

    let tag = token
        .map(|t| compute_auth_tag(t, &format!("{advertise}:{engines_csv}:{cpus}:{mem_total_mb}")))
        .unwrap_or_else(|| "-".to_string());

    format!(
        "{NODE_PREFIX}{tag} {advertise} {engines_csv} {rss_mb} {load_per_cpu:.4} {cpus} {mem_total_mb} {mem_avail_mb} {sessions} {ws}"
    )
}

// ── Gateway side ──────────────────────────────────────────────────────────────

/// Bind the discovery UDP socket.
pub fn bind_discovery_socket() -> std::io::Result<UdpSocket> {
    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    sock.set_reuse_address(true)?;
    #[cfg(unix)]
    sock.set_reuse_port(true)?;
    sock.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT).into())?;
    sock.join_multicast_v4(&MULTICAST_GROUP, &Ipv4Addr::UNSPECIFIED)?;
    sock.set_multicast_loop_v4(false)?;
    sock.set_nonblocking(true)?;
    Ok(sock.into())
}

/// Check whether an incoming probe is valid and authorized.
pub fn is_valid_probe(buf: &[u8], token: Option<&str>) -> bool {
    if !buf.starts_with(PROBE_PREFIX.as_bytes()) {
        return false;
    }
    if let Some(token) = token {
        let text = match std::str::from_utf8(buf) {
            Ok(s) => s.trim(),
            Err(_) => return false,
        };
        let mut parts = text.split_whitespace();
        let _ = parts.next(); // PROD_CODE_DISCOVER
        let tag = parts.next().unwrap_or("");
        verify_auth_tag(token, PROBE_PREFIX, tag)
    } else {
        true
    }
}

/// Format the probe datagram payload.
pub fn format_probe(token: Option<&str>) -> Vec<u8> {
    if let Some(token) = token {
        let tag = compute_auth_tag(token, PROBE_PREFIX);
        format!("{PROBE_PREFIX} {tag}\n").into_bytes()
    } else {
        format!("{PROBE_PREFIX}\n").into_bytes()
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

/// Discover nodes by sending probes and collecting authenticated replies.
pub fn discover(seeds: &[SocketAddr]) -> Vec<DiscoveredNode> {
    discover_with_token(seeds, crate::transport::auth_token().as_deref())
}

/// Discover nodes with an explicit cluster auth token.
pub fn discover_with_token(seeds: &[SocketAddr], token: Option<&str>) -> Vec<DiscoveredNode> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(COLLECT_TIMEOUT));

    let probe_payload = format_probe(token);

    // Send probes.
    let mcast_dest = SocketAddrV4::new(MULTICAST_GROUP, DISCOVERY_PORT);
    let _ = sock.send_to(&probe_payload, mcast_dest);
    for seed in seeds {
        let unicast_dest = SocketAddr::new(seed.ip(), DISCOVERY_PORT);
        let _ = sock.send_to(&probe_payload, unicast_dest);
    }

    // Collect replies.
    let mut nodes: HashMap<SocketAddr, DiscoveredNode> = HashMap::new();
    let deadline = Instant::now() + COLLECT_TIMEOUT;
    let mut buf = [0u8; MAX_DGRAM];
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let _ = sock.set_read_timeout(Some(remaining.max(Duration::from_millis(1))));
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if let Ok(text) = std::str::from_utf8(&buf[..n]) {
                    for line in text.lines() {
                        if let Some(node) = parse_node_line_with_auth(line, token, Some(from.ip())) {
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
            32, 128000, 64000, 3, &ws, None,
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
    fn workspace_names_with_spaces_and_delimiters_are_safely_preserved() {
        let ws = vec![
            ("My Project with spaces".into(), "rust".into(), 1),
            ("repo,with,commas:and:colons".into(), "go".into(), 2),
            ("русский проект".into(), "python".into(), 0),
        ];
        let line = format_node_line(
            "10.0.0.1:9400", "rust,go,python", 500, 0.1,
            8, 16000, 8000, 3, &ws, None,
        );
        let node = parse_node_line(&line).expect("should parse despite spaces and colons in names");
        assert_eq!(node.workspaces.len(), 3);
        assert_eq!(node.workspaces[0].name, "My Project with spaces");
        assert_eq!(node.workspaces[1].name, "repo,with,commas:and:colons");
        assert_eq!(node.workspaces[2].name, "русский проект");
    }

    #[test]
    fn authenticated_announcement_round_trips() {
        let token = "secret-cluster-token-12345";
        let line = format_node_line(
            "192.168.2.100:9400", "rust", 200, 0.05,
            4, 8000, 4000, 1, &[], Some(token),
        );
        let sender_ip: IpAddr = "192.168.2.100".parse().unwrap();
        let node = parse_node_line_with_auth(&line, Some(token), Some(sender_ip))
            .expect("should accept valid authenticated announcement");
        assert_eq!(node.addr, "192.168.2.100:9400".parse().unwrap());
    }

    #[test]
    fn forged_announcement_rejected_when_token_configured() {
        let token = "secret-cluster-token-12345";
        // Unauthenticated line (tag = -)
        let unauth_line = format_node_line(
            "192.168.2.100:9400", "rust", 200, 0.05,
            4, 8000, 4000, 1, &[], None,
        );
        let sender_ip: IpAddr = "192.168.2.100".parse().unwrap();
        assert!(
            parse_node_line_with_auth(&unauth_line, Some(token), Some(sender_ip)).is_none(),
            "must reject unauthenticated announce when token is configured"
        );

        // Forged line with wrong token
        let wrong_token_line = format_node_line(
            "192.168.2.100:9400", "rust", 200, 0.05,
            4, 8000, 4000, 1, &[], Some("wrong-token"),
        );
        assert!(
            parse_node_line_with_auth(&wrong_token_line, Some(token), Some(sender_ip)).is_none(),
            "must reject announce with wrong token"
        );
    }

    #[test]
    fn anti_spoofing_rejects_mismatched_sender_ip() {
        let line = format_node_line(
            "192.168.2.168:9400", "rust", 200, 0.05,
            4, 8000, 4000, 1, &[], None,
        );
        // Sender IP is attacker at 192.168.2.99 pretending to advertise 192.168.2.168
        let attacker_ip: IpAddr = "192.168.2.99".parse().unwrap();
        assert!(
            parse_node_line_with_auth(&line, None, Some(attacker_ip)).is_none(),
            "must reject spoofed sender IP"
        );

        // Legitimate sender matching advertised IP is accepted
        let real_ip: IpAddr = "192.168.2.168".parse().unwrap();
        assert!(parse_node_line_with_auth(&line, None, Some(real_ip)).is_some());
    }

    #[test]
    fn probe_validation_with_token() {
        let token = "my-secret-token";
        let valid_probe = format_probe(Some(token));
        assert!(is_valid_probe(&valid_probe, Some(token)));

        let unauth_probe = b"PROD_CODE_DISCOVER\n";
        assert!(!is_valid_probe(unauth_probe, Some(token)));

        let wrong_token_probe = format_probe(Some("wrong-token"));
        assert!(!is_valid_probe(&wrong_token_probe, Some(token)));
    }

    #[test]
    fn parse_minimal_legacy_line() {
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
}
