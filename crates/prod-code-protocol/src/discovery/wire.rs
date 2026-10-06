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
    DISCOVERY_PORT, DiscoveredNode, LoadedWorkspace, MULTICAST_GROUP, NODE_PREFIX, PROBE_PREFIX,
    canonical_announce_payload_with_nonce, compute_auth_tag, decode_workspace_name,
    encode_workspace_name, verify_auth_tag,
};
use std::net::{IpAddr, SocketAddr, SocketAddrV4, UdpSocket};

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
        && nonce.as_deref() != Some(exp_n)
    {
        tracing::warn!(addr = %addr, expected = %exp_n, got = ?nonce, "rejecting replayed or mismatched discovery reply");
        return None;
    }

    // Auth verification: if an auth token is configured, reject announcements with invalid MAC tag.
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
pub fn format_minimal_node_line(advertise: &str, engines_csv: &str, token: Option<&str>) -> String {
    format_node_line_with_nonce(advertise, engines_csv, 0, 0.0, 0, 0, 0, 0, &[], token, None)
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
        (None, Some(nonce)) => format!("{PROBE_PREFIX} - {nonce}\n").into_bytes(),
        (None, None) => format!("{PROBE_PREFIX}\n").into_bytes(),
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
