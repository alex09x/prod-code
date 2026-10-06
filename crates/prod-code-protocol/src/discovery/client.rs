/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{COLLECT_TIMEOUT, DISCOVERY_PORT, DiscoveredNode, MAX_DGRAM, MULTICAST_GROUP};
use super::wire::{format_probe_with_nonce, parse_node_line_with_auth_and_nonce};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

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
