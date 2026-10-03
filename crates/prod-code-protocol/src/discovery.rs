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

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Computes a standard 256-bit HMAC-SHA-256 authentication tag, hex-encoded (64 characters).
pub fn compute_auth_tag(token: &str, data: &str) -> String {
    let mut mac = match HmacSha256::new_from_slice(token.as_bytes()) {
        Ok(m) => m,
        Err(_) => return String::new(),
    };
    mac.update(data.as_bytes());
    let result = mac.finalize();
    let bytes = result.into_bytes();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// Verifies an HMAC-SHA-256 auth tag against the expected token in constant time.
pub fn verify_auth_tag(token: &str, data: &str, tag: &str) -> bool {
    let mut mac = match HmacSha256::new_from_slice(token.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(data.as_bytes());

    // Parse tag into bytes.
    if tag.len() != 64 {
        return false;
    }
    let mut tag_bytes = [0u8; 32];
    let (chunks, _) = tag.as_bytes().as_chunks::<2>();
    for (i, chunk) in chunks.iter().enumerate() {
        let Ok(s) = std::str::from_utf8(chunk) else {
            return false;
        };
        let Ok(b) = u8::from_str_radix(s, 16) else {
            return false;
        };
        tag_bytes[i] = b;
    }

    // verify_slice provides constant-time comparison against timing attacks.
    mac.verify_slice(&tag_bytes).is_ok()
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
    /// Optional challenge nonce echoed in reply to a unicast probe.
    pub nonce: Option<String>,
}

/// Canonical payload string for HMAC-SHA-256 signing/verification of node announcements with optional challenge nonce.
/// Covers ALL wire fields affecting node identity, eligibility, warm-workspace preference, scoring, and freshness:
/// `<addr> <engines_csv> <rss_mb> <load_per_cpu:.4> <cpus> <mem_total_mb> <mem_avail_mb> <sessions> <ws_csv> [<nonce>]`
#[allow(clippy::too_many_arguments)]
pub fn canonical_announce_payload_with_nonce(
    advertise: &str,
    engines_csv: &str,
    rss_mb: u64,
    load_per_cpu: f64,
    cpus: u32,
    mem_total_mb: u64,
    mem_avail_mb: u64,
    sessions: u32,
    ws_csv: &str,
    nonce: Option<&str>,
) -> String {
    if let Some(n) = nonce {
        format!(
            "{advertise} {engines_csv} {rss_mb} {load_per_cpu:.4} {cpus} {mem_total_mb} {mem_avail_mb} {sessions} {ws_csv} {n}"
        )
    } else {
        format!(
            "{advertise} {engines_csv} {rss_mb} {load_per_cpu:.4} {cpus} {mem_total_mb} {mem_avail_mb} {sessions} {ws_csv}"
        )
    }
}

/// Canonical payload string without nonce (backward-compatible).
#[allow(clippy::too_many_arguments)]
pub fn canonical_announce_payload(
    advertise: &str,
    engines_csv: &str,
    rss_mb: u64,
    load_per_cpu: f64,
    cpus: u32,
    mem_total_mb: u64,
    mem_avail_mb: u64,
    sessions: u32,
    ws_csv: &str,
) -> String {
    canonical_announce_payload_with_nonce(
        advertise,
        engines_csv,
        rss_mb,
        load_per_cpu,
        cpus,
        mem_total_mb,
        mem_avail_mb,
        sessions,
        ws_csv,
        None,
    )
}

/// Parse one `PROD_CODE_NODE` line, validating anti-spoofing, auth token MAC, and optional expected challenge nonce.
pub fn parse_node_line_with_auth_and_nonce(
    line: &str,
    expected_token: Option<&str>,
    sender_ip: Option<IpAddr>,
    expected_nonce: Option<&str>,
) -> Option<DiscoveredNode> {
    let rest = line.strip_prefix(NODE_PREFIX)?;
    let mut parts = rest.split_whitespace();

    let first = parts.next()?;
    // Support backward-compatible lines where field 0 is addr, or new lines where field 0 is tag.
    let (tag, addr_str) = if first.parse::<SocketAddr>().is_ok() {
        ("-", first)
    } else {
        (first, parts.next()?)
    };

    let addr: SocketAddr = addr_str.parse().ok()?;

    // Anti-spoofing check: advertised IP must match actual UDP packet sender IP
    // (unless one is loopback, as in local tests).
    if let Some(from_ip) = sender_ip
        && !from_ip.is_loopback()
        && !addr.ip().is_loopback()
        && from_ip != addr.ip()
    {
        tracing::warn!(from = %from_ip, advertised = %addr, "rejecting spoofed discovery advertisement");
        return None;
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
    let ws_csv = parts.next().unwrap_or("-");
    let nonce = parts.next().map(|s| s.to_string());

    // If a specific challenge nonce is required, reject replies without or with mismatched nonce.
    if let Some(exp_n) = expected_nonce
        && nonce.as_deref() != Some(exp_n) {
            tracing::warn!(addr = %addr, expected = %exp_n, got = ?nonce, "rejecting replayed or mismatched discovery reply");
            return None;
        }

    // Auth verification: if an auth token is configured, reject announcements with invalid MAC tag.
    // The MAC covers all routing and metadata fields on the wire plus the nonce if present.
    if let Some(token) = expected_token {
        let canonical_payload = canonical_announce_payload_with_nonce(
            addr_str,
            engines_csv,
            rss_mb,
            load_per_cpu,
            cpus,
            mem_total_mb,
            mem_avail_mb,
            sessions,
            ws_csv,
            nonce.as_deref(),
        );
        if !verify_auth_tag(token, &canonical_payload, tag) {
            tracing::warn!(addr = %addr, "rejecting unauthenticated/tampered discovery advertisement");
            return None;
        }
    }

    let workspaces = if ws_csv == "-" {
        Vec::new()
    } else {
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
    };

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
        nonce,
    })
}

/// Parse one `PROD_CODE_NODE` line, validating anti-spoofing and auth token MAC.
pub fn parse_node_line_with_auth(
    line: &str,
    expected_token: Option<&str>,
    sender_ip: Option<IpAddr>,
) -> Option<DiscoveredNode> {
    parse_node_line_with_auth_and_nonce(line, expected_token, sender_ip, None)
}

/// Convenience parser without anti-spoofing or auth verification (used by unit tests).
pub fn parse_node_line(line: &str) -> Option<DiscoveredNode> {
    parse_node_line_with_auth(line, None, None)
}

/// Format one announce line with optional challenge nonce.
#[allow(clippy::too_many_arguments)]
pub fn format_node_line_with_nonce(
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
    nonce: Option<&str>,
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

    let canonical = canonical_announce_payload_with_nonce(
        advertise,
        engines_csv,
        rss_mb,
        load_per_cpu,
        cpus,
        mem_total_mb,
        mem_avail_mb,
        sessions,
        &ws,
        nonce,
    );

    let tag = token
        .map(|t| compute_auth_tag(t, &canonical))
        .unwrap_or_else(|| "-".to_string());

    format!("{NODE_PREFIX}{tag} {canonical}")
}

/// Format one announce line (without trailing newline).
#[allow(clippy::too_many_arguments)]
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
    format_node_line_with_nonce(
        advertise,
        engines_csv,
        rss_mb,
        load_per_cpu,
        cpus,
        mem_total_mb,
        mem_avail_mb,
        sessions,
        workspaces,
        token,
        None,
    )
}

/// Format a minimal node announcement line for discovery privacy (Phase 5.6).
/// Strips internal host telemetry (RSS, load, CPUs, RAM, sessions) and loaded workspace names.
pub fn format_minimal_node_line(
    advertise: &str,
    engines_csv: &str,
    token: Option<&str>,
) -> String {
    format_node_line_with_nonce(
        advertise,
        engines_csv,
        0,
        0.0,
        0,
        0,
        0,
        0,
        &[],
        token,
        None,
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

/// Generate a cryptographically secure 128-bit hex nonce for discovery challenges.
///
/// Fails closed with an error if the secure random number generator is unavailable.
pub fn generate_nonce() -> std::io::Result<String> {
    let mut buf = [0u8; 16];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut buf)
        .map_err(|_| {
            std::io::Error::other(
                "cryptographically secure random number generator (ring) failed to generate nonce",
            )
        })?;
    let mut s = String::with_capacity(32);
    for b in buf {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    Ok(s)
}

/// Format the probe datagram payload with optional replay-protection challenge nonce.
pub fn format_probe_with_nonce(token: Option<&str>, nonce: Option<&str>) -> Vec<u8> {
    match (token, nonce) {
        (Some(token), Some(nonce)) => {
            let data = format!("{PROBE_PREFIX} {nonce}");
            let tag = compute_auth_tag(token, &data);
            format!("{PROBE_PREFIX} {tag} {nonce}\n").into_bytes()
        }
        (Some(token), None) => {
            let tag = compute_auth_tag(token, PROBE_PREFIX);
            format!("{PROBE_PREFIX} {tag}\n").into_bytes()
        }
        (None, Some(nonce)) => {
            format!("{PROBE_PREFIX} - {nonce}\n").into_bytes()
        }
        (None, None) => {
            format!("{PROBE_PREFIX}\n").into_bytes()
        }
    }
}

/// Format the probe datagram payload without challenge nonce.
pub fn format_probe(token: Option<&str>) -> Vec<u8> {
    format_probe_with_nonce(token, None)
}

/// Inspect incoming probe datagram, verifying authentication and extracting any challenge nonce.
pub fn inspect_probe(buf: &[u8], token: Option<&str>) -> Option<Option<String>> {
    if !buf.starts_with(PROBE_PREFIX.as_bytes()) {
        return None;
    }
    let text = match std::str::from_utf8(buf) {
        Ok(s) => s.trim(),
        Err(_) => return None,
    };
    let mut parts = text.split_whitespace();
    let _ = parts.next(); // PROD_CODE_DISCOVER
    let tag = parts.next();
    let nonce = parts.next().map(|s| s.to_string());

    if let Some(token) = token {
        let tag = tag?;
        let expected_data = if let Some(n) = &nonce {
            format!("{PROBE_PREFIX} {n}")
        } else {
            PROBE_PREFIX.to_string()
        };
        if !verify_auth_tag(token, &expected_data, tag) {
            return None;
        }
    }
    Some(nonce)
}

/// Check whether an incoming probe is valid and authorized.
pub fn is_valid_probe(buf: &[u8], token: Option<&str>) -> bool {
    inspect_probe(buf, token).is_some()
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

/// Discover nodes with an explicit cluster auth token and replay-resistant challenge nonce verification.
pub fn discover_with_token(seeds: &[SocketAddr], token: Option<&str>) -> Vec<DiscoveredNode> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(COLLECT_TIMEOUT));

    let challenge_nonce = match generate_nonce() {
        Ok(n) => n,
        Err(e) => {
            tracing::error!(%e, "cannot discover nodes: secure random number generator failed");
            return Vec::new();
        }
    };
    let probe_payload = format_probe_with_nonce(token, Some(&challenge_nonce));

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
                        // Replay protection: require the exact challenge nonce on unicast probe replies
                        if let Some(node) = parse_node_line_with_auth_and_nonce(
                            line,
                            token,
                            Some(from.ip()),
                            Some(&challenge_nonce),
                        ) {
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

    #[test]
    fn hmac_sha256_mac_computation_and_verification() {
        let token = "test-cluster-secret-key-32bytes!";
        let data = "192.168.2.168:9400:rust,go:32:128000";

        let tag = compute_auth_tag(token, data);
        assert_eq!(tag.len(), 64, "HMAC-SHA-256 hex string must be 64 characters (256 bits)");

        // Valid verification
        assert!(verify_auth_tag(token, data, &tag));

        // Wrong token fails
        assert!(!verify_auth_tag("different-token", data, &tag));

        // Tampered payload fails
        assert!(!verify_auth_tag(token, "192.168.2.168:9400:rust,go:32:128001", &tag));

        // Tampered tag (single bit flip) fails
        let mut tampered_tag = tag.clone();
        let last_char = if tampered_tag.ends_with('0') { '1' } else { '0' };
        tampered_tag.pop();
        tampered_tag.push(last_char);
        assert!(!verify_auth_tag(token, data, &tampered_tag));

        // Truncated / malformed tag fails
        assert!(!verify_auth_tag(token, data, &tag[..32]));
        assert!(!verify_auth_tag(token, data, "invalid-hex-characters-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"));
    }

    #[test]
    fn tampering_any_announcement_field_fails_verification() {
        let token = "secret-cluster-token-987654";
        let ws = vec![("my-app".into(), "rust".into(), 1)];
        let valid_line = format_node_line(
            "192.168.2.168:9400", "rust,go", 1000, 0.05,
            16, 64000, 32000, 2, &ws, Some(token),
        );
        let sender_ip: IpAddr = "192.168.2.168".parse().unwrap();

        // 1. Valid line must pass
        assert!(parse_node_line_with_auth(&valid_line, Some(token), Some(sender_ip)).is_some());

        let tokens: Vec<&str> = valid_line.split_whitespace().collect();
        // Wire tokens:
        // [0]: "PROD_CODE_NODE"
        // [1]: <64-char HMAC tag>
        // [2]: "192.168.2.168:9400" (addr)
        // [3]: "rust,go" (engines)
        // [4]: "1000" (rss_mb)
        // [5]: "0.0500" (load_per_cpu)
        // [6]: "16" (cpus)
        // [7]: "64000" (mem_total_mb)
        // [8]: "32000" (mem_avail_mb)
        // [9]: "2" (sessions)
        // [10]: "my-app:rust:1" (workspaces)

        // 2. Tampering with engines
        let mut tampered = tokens.clone();
        tampered[3] = "rust,go,python";
        assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

        // 3. Tampering with rss_mb
        let mut tampered = tokens.clone();
        tampered[4] = "2000";
        assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

        // 4. Tampering with load_per_cpu
        let mut tampered = tokens.clone();
        tampered[5] = "0.0100";
        assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

        // 5. Tampering with cpus
        let mut tampered = tokens.clone();
        tampered[6] = "32";
        assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

        // 6. Tampering with mem_total_mb
        let mut tampered = tokens.clone();
        tampered[7] = "128000";
        assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

        // 7. Tampering with mem_avail_mb (routing priority!)
        let mut tampered = tokens.clone();
        tampered[8] = "60000";
        assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

        // 8. Tampering with sessions
        let mut tampered = tokens.clone();
        tampered[9] = "10";
        assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

        // 9. Tampering with workspaces (warm-cache routing priority!)
        let mut tampered = tokens.clone();
        tampered[10] = "other-app:rust:1";
        assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());
    }

    #[test]
    fn minimal_node_announcement_satisfies_privacy_and_parses() {
        let token = "privacy-token-secret";
        let line = format_minimal_node_line("192.168.2.168:9400", "rust,go", Some(token));
        let sender_ip: IpAddr = "192.168.2.168".parse().unwrap();
        let node = parse_node_line_with_auth(&line, Some(token), Some(sender_ip))
            .expect("minimal announcement should parse and authenticate");

        assert_eq!(node.addr, "192.168.2.168:9400".parse().unwrap());
        assert_eq!(node.engines, vec!["rust", "go"]);
        assert_eq!(node.rss_mb, 0);
        assert_eq!(node.load_per_cpu, 0.0);
        assert_eq!(node.cpus, 0);
        assert_eq!(node.mem_total_mb, 0);
        assert_eq!(node.mem_avail_mb, 0);
        assert_eq!(node.sessions, 0);
        assert!(node.workspaces.is_empty(), "workspaces must be stripped for privacy");
        assert!(node.nonce.is_none());
    }

    #[test]
    fn challenge_nonce_probe_and_reply_verification() {
        let token = "test-token-nonce";
        let nonce = generate_nonce().unwrap();
        assert_eq!(nonce.len(), 32);

        // Probe formatting and inspection
        let probe = format_probe_with_nonce(Some(token), Some(&nonce));
        let inspected = inspect_probe(&probe, Some(token)).expect("probe must be valid");
        assert_eq!(inspected.as_deref(), Some(nonce.as_str()));

        // Reply formatting with nonce
        let ws = vec![("secure-project".into(), "rust".into(), 1)];
        let reply_line = format_node_line_with_nonce(
            "192.168.2.168:9400", "rust", 500, 0.1,
            8, 16000, 8000, 1, &ws, Some(token), Some(&nonce),
        );
        let sender_ip: IpAddr = "192.168.2.168".parse().unwrap();

        // Valid reply with matching nonce is accepted
        let node = parse_node_line_with_auth_and_nonce(&reply_line, Some(token), Some(sender_ip), Some(&nonce))
            .expect("should accept valid reply with matching nonce");
        assert_eq!(node.nonce.as_deref(), Some(nonce.as_str()));
        assert_eq!(node.workspaces.len(), 1);
        assert_eq!(node.workspaces[0].name, "secure-project");

        // Reply with wrong expected nonce is rejected
        let different_nonce = generate_nonce().unwrap();
        assert!(
            parse_node_line_with_auth_and_nonce(&reply_line, Some(token), Some(sender_ip), Some(&different_nonce)).is_none(),
            "must reject when expected nonce differs from wire nonce"
        );
    }

    #[test]
    fn replayed_announcement_without_nonce_fails_when_nonce_expected() {
        let token = "test-token-replay";
        let old_line = format_node_line(
            "192.168.2.168:9400", "rust", 500, 0.1,
            8, 16000, 8000, 1, &[], Some(token),
        );
        let sender_ip: IpAddr = "192.168.2.168".parse().unwrap();
        let fresh_nonce = generate_nonce().unwrap();

        assert!(
            parse_node_line_with_auth_and_nonce(&old_line, Some(token), Some(sender_ip), Some(&fresh_nonce)).is_none(),
            "must reject replayed line that lacks fresh challenge nonce"
        );
    }

    #[test]
    fn replayed_signed_reply_with_empty_workspaces_fails_without_nonce() {
        let token = "test-token-empty-ws";
        // Node with empty workspaces: ws_csv is "-"
        let empty_ws_reply = format_node_line(
            "192.168.2.168:9400", "rust", 500, 0.1,
            8, 16000, 8000, 0, &[], Some(token),
        );
        let sender_ip: IpAddr = "192.168.2.168".parse().unwrap();
        let fresh_nonce = generate_nonce().unwrap();

        assert!(
            parse_node_line_with_auth_and_nonce(&empty_ws_reply, Some(token), Some(sender_ip), Some(&fresh_nonce)).is_none(),
            "must reject replayed signed reply even when workspaces are empty"
        );
    }
}
