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

#[cfg(test)]
#[path = "placement_tests.rs"]
mod tests;

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Placement {
    #[serde(default)]
    pub(crate) workspaces: BTreeMap<String, SocketAddr>,
}

pub(crate) fn placement_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/share/prod_code/placement.json"))
}

static PLACEMENT_FILE_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn placement_lock() -> &'static std::sync::Mutex<()> {
    PLACEMENT_FILE_LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

pub(crate) fn load_placement(path: &Path) -> Placement {
    let _guard = placement_lock().lock().unwrap_or_else(|e| e.into_inner());
    load_placement_unlocked(path)
}

fn load_placement_unlocked(path: &Path) -> Placement {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn save_placement(path: &Path, placement: &Placement) {
    let _guard = placement_lock().lock().unwrap_or_else(|e| e.into_inner());
    save_placement_unlocked(path, placement);
}

fn save_placement_unlocked(path: &Path, placement: &Placement) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(placement) {
        let seq = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp_name = format!(
            ".{}.tmp.{}.{}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("placement.json"),
            std::process::id(),
            seq
        );
        let tmp_path = path.with_file_name(tmp_name);
        if std::fs::write(&tmp_path, &bytes).is_ok() {
            if std::fs::rename(&tmp_path, path).is_err() {
                let _ = std::fs::write(path, &bytes);
                let _ = std::fs::remove_file(&tmp_path);
            }
        } else {
            let _ = std::fs::write(path, &bytes);
        }
    }
}

/// Remember the chosen node for `workspace_name` in the placement file.
pub fn remember_placement(workspace_name: &str, node: SocketAddr) {
    remember_placement_at(placement_path().as_deref(), workspace_name, node);
}

fn remember_placement_at(path: Option<&Path>, workspace_name: &str, node: SocketAddr) {
    let _guard = placement_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(path) = path {
        let mut placement = load_placement_unlocked(path);
        placement
            .workspaces
            .insert(workspace_name.to_string(), node);
        save_placement_unlocked(path, &placement);
    }
}

/// Remove a stale remembered placement for `workspace_name` from the placement file.
pub fn forget_placement(workspace_name: &str) {
    forget_placement_at(placement_path().as_deref(), workspace_name);
}

fn forget_placement_at(path: Option<&Path>, workspace_name: &str) {
    let _guard = placement_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(path) = path {
        let mut placement = load_placement_unlocked(path);
        if placement.workspaces.remove(workspace_name).is_some() {
            save_placement_unlocked(path, &placement);
        }
    }
}

/// The remembered placement of `workspace_name`, if any.
pub fn remembered_node(workspace_name: &str) -> Option<SocketAddr> {
    let _guard = placement_lock().lock().unwrap_or_else(|e| e.into_inner());
    let path = placement_path()?;
    load_placement_unlocked(&path)
        .workspaces
        .get(workspace_name)
        .copied()
}
