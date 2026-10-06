/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

/// The UDP port used for discovery (one above the gateway TCP port).
pub const DISCOVERY_PORT: u16 = 9401;

/// Multicast group for LAN announcements.
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 80, 67);

/// How often a gateway announces itself on multicast.
pub const ANNOUNCE_PERIOD: Duration = Duration::from_secs(5);

/// How long a client collects multicast/unicast replies before returning.
pub const COLLECT_TIMEOUT: Duration = Duration::from_millis(250);

/// Maximum UDP datagram we handle.
pub(crate) const MAX_DGRAM: usize = 4096;

pub(crate) const PROBE_PREFIX: &str = "PROD_CODE_DISCOVER";
pub(crate) const NODE_PREFIX: &str = "PROD_CODE_NODE ";

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
