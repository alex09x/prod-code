/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Placement {
    #[serde(default)]
    pub(crate) workspaces: BTreeMap<String, SocketAddr>,
}

pub(crate) fn placement_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/prod_code/placement.json"))
}

pub(crate) fn load_placement(path: &Path) -> Placement {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn save_placement(path: &Path, placement: &Placement) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(placement) {
        let _ = std::fs::write(path, bytes);
    }
}

/// Remember the chosen node for `workspace_name` in the placement file.
pub fn remember_placement(workspace_name: &str, node: SocketAddr) {
    if let Some(path) = placement_path() {
        let mut placement = load_placement(&path);
        placement
            .workspaces
            .insert(workspace_name.to_string(), node);
        save_placement(&path, &placement);
    }
}

/// The remembered placement of `workspace_name`, if any.
pub fn remembered_node(workspace_name: &str) -> Option<SocketAddr> {
    let path = placement_path()?;
    load_placement(&path)
        .workspaces
        .get(workspace_name)
        .copied()
}
