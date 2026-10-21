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
use std::path::Path;

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
