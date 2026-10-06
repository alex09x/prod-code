/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::time::Duration;
use tokio::net::TcpStream;

/// How long a connection may be silent before the kernel starts probing the peer, how far apart
/// the probes are, and how many may go unanswered before the connection is reset.
pub const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
pub const KEEPALIVE_RETRIES: u32 = 3;

/// The environment variable that configures custom TCP buffer sizes (bytes or e.g. "4M", "8MB", "16MiB").
/// When unset, socket buffers are left to kernel autotuning.
pub const TCP_BUFFER_SIZE_ENV: &str = "PROD_CODE_TCP_BUFFER_SIZE";

/// The environment variable that configures custom TCP receive buffer size.
pub const TCP_RECV_BUFFER_ENV: &str = "PROD_CODE_TCP_RECV_BUFFER";

/// The environment variable that configures custom TCP send buffer size.
pub const TCP_SEND_BUFFER_ENV: &str = "PROD_CODE_TCP_SEND_BUFFER";

/// Parses a human-readable byte size specification (e.g. "4194304", "512K", "4M", "8MB", "16MiB").
pub fn parse_buffer_size(val: &str) -> Option<usize> {
    let s = val.trim();
    if s.is_empty() {
        return None;
    }
    let (num_part, multiplier) = if let Some(stripped) = s
        .strip_suffix("GiB")
        .or_else(|| s.strip_suffix("gib"))
        .or_else(|| s.strip_suffix("GB"))
        .or_else(|| s.strip_suffix("gb"))
        .or_else(|| s.strip_suffix('G'))
        .or_else(|| s.strip_suffix('g'))
    {
        (stripped.trim(), 1024 * 1024 * 1024)
    } else if let Some(stripped) = s
        .strip_suffix("MiB")
        .or_else(|| s.strip_suffix("mib"))
        .or_else(|| s.strip_suffix("MB"))
        .or_else(|| s.strip_suffix("mb"))
        .or_else(|| s.strip_suffix('M'))
        .or_else(|| s.strip_suffix('m'))
    {
        (stripped.trim(), 1024 * 1024)
    } else if let Some(stripped) = s
        .strip_suffix("KiB")
        .or_else(|| s.strip_suffix("kib"))
        .or_else(|| s.strip_suffix("KB"))
        .or_else(|| s.strip_suffix("kb"))
        .or_else(|| s.strip_suffix('K'))
        .or_else(|| s.strip_suffix('k'))
    {
        (stripped.trim(), 1024)
    } else if let Some(stripped) = s.strip_suffix('B').or_else(|| s.strip_suffix('b')) {
        (stripped.trim(), 1)
    } else {
        (s, 1)
    };

    num_part
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
}

/// Configures socket buffer sizes on `stream` according to explicit options, alongside TCP keepalive and nodelay.
pub fn tune_with_buffer_sizes(
    stream: &TcpStream,
    recv_size: Option<usize>,
    send_size: Option<usize>,
) {
    let _ = stream.set_nodelay(true);
    let sock = socket2::SockRef::from(stream);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL)
        .with_retries(KEEPALIVE_RETRIES);
    let _ = sock.set_tcp_keepalive(&keepalive);

    if let Some(sz) = recv_size {
        let _ = sock.set_recv_buffer_size(sz);
    }
    if let Some(sz) = send_size {
        let _ = sock.set_send_buffer_size(sz);
    }
}

/// Turns Nagle off, TCP keepalive on, and applies custom socket buffer sizes if configured in the environment.
pub fn tune(stream: &TcpStream) {
    let general = std::env::var(TCP_BUFFER_SIZE_ENV)
        .ok()
        .and_then(|v| parse_buffer_size(&v));
    let rcv = std::env::var(TCP_RECV_BUFFER_ENV)
        .ok()
        .and_then(|v| parse_buffer_size(&v))
        .or(general);
    let snd = std::env::var(TCP_SEND_BUFFER_ENV)
        .ok()
        .and_then(|v| parse_buffer_size(&v))
        .or(general);

    tune_with_buffer_sizes(stream, rcv, snd);
}
