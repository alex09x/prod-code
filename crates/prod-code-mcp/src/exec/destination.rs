/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::cluster::NodePolicy;

// Execution provenance is per checkout and must survive analyzer placement changes.
fn key(root: &Path) -> String {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    format!("exec:{}", canonical.display())
}

pub fn remember_exec_node(root: &Path, node: SocketAddr) {
    crate::cluster::remember_placement(&key(root), node);
}

pub fn last_exec_node(root: &Path) -> Option<SocketAddr> {
    crate::cluster::remembered_node(&key(root))
}

/// The node artifacts of the checkout at `root` are pulled from (#1793): `requested` when given,
/// otherwise the node its last command ran on, otherwise `current`, and in every case only a node
/// the checkout's pin allows. A remembered node outside the pin (from before it) is passed over;
/// a requested one is refused.
pub fn pull_node(root: &Path, current: SocketAddr, requested: Option<&str>) -> Result<SocketAddr> {
    let policy = crate::cluster::policy_for_root(root)?;
    pull_node_with(
        &policy,
        current,
        requested,
        last_exec_node(root),
        |spec: &str| spec.to_socket_addrs().map(Iterator::collect),
    )
}

/// [`pull_node`] with the policy, the remembered node and the resolver given. A requested name
/// may resolve to several addresses; the first the policy allows is used, so another address of
/// the pinned endpoint works and nothing outside it does.
pub fn pull_node_with(
    policy: &NodePolicy,
    current: SocketAddr,
    requested: Option<&str>,
    last: Option<SocketAddr>,
    resolve: impl FnOnce(&str) -> std::io::Result<Vec<SocketAddr>>,
) -> Result<SocketAddr> {
    if let Some(spec) = requested {
        let spec = spec.trim();
        let addrs = match spec.parse::<SocketAddr>() {
            Ok(addr) => vec![addr],
            Err(_) => resolve(spec)
                .with_context(|| format!("remote must be a node address (HOST:PORT): {spec}"))?,
        };
        if let Some(addr) = addrs.iter().copied().find(|addr| policy.allows(*addr)) {
            return Ok(addr);
        }
        if addrs.is_empty() {
            bail!("remote {spec} resolved to no address");
        }
        bail!(
            "cannot pull from {spec}: this workspace is {}, and none of its addresses ({}) is \
             allowed; pull from the pinned node, or run unpinned with `--pin-node auto`",
            policy.describe(),
            addrs
                .iter()
                .map(SocketAddr::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if let Some(last) = last.filter(|node| policy.allows(*node)) {
        return Ok(last);
    }
    crate::cluster::place_within(policy, current)
}
