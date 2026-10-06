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
use std::time::Duration;

use prod_code_protocol::{StatusResponse, content_hash};

use super::discover::node_status;
use super::routing::PROBE_TIMEOUT;

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
pub(crate) const RESTART_RETRIES: usize = 4;
pub(crate) const RESTART_WAIT: Duration = Duration::from_millis(750);

/// Can `node` take the workspace: alive, serving `engine` and running `os` when they are needed.
pub(crate) async fn node_fits(node: SocketAddr, engine: Option<&str>, os: Option<&str>) -> bool {
    if engine.is_none() && os.is_none() {
        return is_alive(node).await;
    }
    node_status(node)
        .await
        .is_ok_and(|status| status_fits(&status, engine, os))
}

/// Whether a gateway with `status` serves `engine` and runs `os`, those that are given.
pub(crate) fn status_fits(status: &StatusResponse, engine: Option<&str>, os: Option<&str>) -> bool {
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
pub(crate) fn os_name(os: &str) -> &str {
    match os {
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
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
