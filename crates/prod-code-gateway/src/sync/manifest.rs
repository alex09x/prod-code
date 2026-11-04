/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    io,
    path::Path,
};

use crate::seed_cache::build_cache::walk_files;
use prod_code_protocol::{FileStamp, content_hash};

const MANIFEST_PATH: &str = ".prod/client-sync-manifest.json";

pub(super) fn load(root: &Path) -> BTreeSet<String> {
    std::fs::read(root.join(MANIFEST_PATH))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(super) fn save(root: &Path, paths: &BTreeSet<String>) -> io::Result<()> {
    let path = root.join(MANIFEST_PATH);
    let parent = path.parent().expect("manifest path has a parent");
    std::fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec(paths).map_err(io::Error::other)?;
    let temp = parent.join("client-sync-manifest.json.tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(temp, path)
}

pub fn reconcile_manifest(root: &Path, stamps: &[FileStamp]) -> (Vec<String>, Vec<String>) {
    let mut present = Vec::new();
    walk_files(root, root, &mut present);
    let managed = present.into_iter().map(|(path, _)| path).collect();
    reconcile_manifest_with_ownership(root, stamps, &managed)
}

pub fn reconcile_manifest_with_ownership(
    root: &Path,
    stamps: &[FileStamp],
    previously_managed: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let active_exec = crate::workspace::active_exec_snapshot(root);
    let synced_during_exec = active_exec
        .as_ref()
        .map(|(started, _)| crate::workspace::synced_since(root, *started))
        .unwrap_or_default();
    let wanted: HashMap<&str, &FileStamp> = stamps
        .iter()
        .map(|s| (s.relative_path.as_str(), s))
        .collect();
    let mut present = Vec::new();
    walk_files(root, root, &mut present);
    let mut missing = Vec::new();
    let mut deleted = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for (rel, path) in &present {
        match wanted.get(rel.as_str()) {
            None => {
                if !previously_managed.contains(rel) {
                    continue;
                }
                if active_exec
                    .as_ref()
                    .is_some_and(|(_, before)| !before.contains_key(rel.as_str()))
                    && !synced_during_exec.contains_key(rel.as_str())
                {
                    continue;
                }
                if std::fs::remove_file(path).is_ok() {
                    let _ = crate::sync::config_meta::forget_sync(root, path);
                    deleted.push(rel.clone());
                }
            }
            Some(stamp) => {
                seen.insert(stamp.relative_path.as_str());
                let same = std::fs::read(path).is_ok_and(|bytes| {
                    let server_size = bytes.len() as u64;
                    let server_hash = content_hash(&bytes);
                    (server_size == stamp.size && server_hash == stamp.hash)
                        || (active_exec.as_ref().is_some_and(|(_, before)| {
                            before.get(rel.as_str()) == Some(&(stamp.size, stamp.hash))
                        }) && !synced_during_exec.contains_key(rel.as_str()))
                        || crate::sync::config_meta::matches(
                            root,
                            path,
                            stamp.size,
                            stamp.hash,
                            server_size,
                            server_hash,
                        )
                });
                let same_mode = stamp
                    .is_executable
                    .is_none_or(|wanted| crate::sync::file_write::is_executable(path) == wanted);
                if !same || !same_mode {
                    missing.push(rel.clone());
                }
            }
        }
    }
    for stamp in stamps {
        if !seen.contains(stamp.relative_path.as_str()) {
            missing.push(stamp.relative_path.clone());
        }
    }
    crate::sync::sync_fs::prune_empty_dirs(root, root);
    missing.sort();
    missing.dedup();
    (missing, deleted)
}
