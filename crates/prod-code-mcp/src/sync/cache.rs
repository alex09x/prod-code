/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::entry::stable_hash;
use crate::sync::types::{SyncCache, WorkspaceIdentity};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Derives the server workspace identity of `dir`.
pub fn workspace_identity(dir: &Path) -> WorkspaceIdentity {
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let own_name = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
        .to_string();
    let dot_git = canonical.join(".git");
    if dot_git.is_file()
        && let Ok(content) = std::fs::read_to_string(&dot_git)
    {
        for line in content.lines() {
            let Some(gitdir) = line.trim().strip_prefix("gitdir:") else {
                continue;
            };
            let gitdir_path = PathBuf::from(gitdir.trim());
            let mut cur = gitdir_path.as_path();
            while let Some(parent) = cur.parent() {
                if cur.file_name().is_some_and(|n| n == ".git")
                    && let Some(origin) = parent.file_name().and_then(|n| n.to_str())
                {
                    let hash = stable_hash(canonical.to_string_lossy().as_bytes()) as u32;
                    return WorkspaceIdentity {
                        name: format!("{origin}--wt-{hash:08x}"),
                        base: Some(origin.to_string()),
                    };
                }
                cur = parent;
            }
        }
    }
    WorkspaceIdentity {
        name: own_name,
        base: None,
    }
}

fn cache_dir() -> PathBuf {
    let cache_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".local/share/prod_code/sync"))
        .unwrap_or_else(|| std::env::temp_dir().join("prod_code_sync_cache"));
    let _ = std::fs::create_dir_all(&cache_dir);
    cache_dir
}

fn worktree_cache_id(root: &Path) -> String {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    format!(
        "{:016x}",
        stable_hash(canonical_root.to_string_lossy().as_bytes())
    )
}

/// The watermark file for `root` as seen by gateway `node` (`host:port`; empty for the
/// node-less legacy watermark used by tests). Every gateway holds its own copy of the
/// workspace, so what has been uploaded is a per-node fact.
fn cache_file_path(root: &Path, node: &str) -> PathBuf {
    let id = worktree_cache_id(root);
    if node.is_empty() {
        cache_dir().join(format!("{id}.json"))
    } else {
        cache_dir().join(format!("{id}-{:016x}.json", stable_hash(node.as_bytes())))
    }
}

/// All watermark files recorded for `root`, one per gateway node (plus the legacy one).
fn cache_file_paths(root: &Path) -> Vec<PathBuf> {
    let id = worktree_cache_id(root);
    let dir = cache_dir();
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".json")
                && (name == format!("{id}.json") || name.starts_with(&format!("{id}-")))
            {
                paths.push(entry.path());
            }
        }
    }
    paths
}

pub fn load_sync_cache(root: &Path) -> SyncCache {
    load_sync_cache_for(root, "")
}

pub fn load_sync_cache_for(root: &Path, node: &str) -> SyncCache {
    let path = cache_file_path(root, node);
    if let Ok(data) = std::fs::read(&path)
        && let Ok(cache) = serde_json::from_slice::<SyncCache>(&data)
    {
        return cache;
    }
    SyncCache::default()
}

pub fn save_sync_cache(root: &Path, cache: &SyncCache) {
    save_sync_cache_for(root, "", cache)
}

pub fn save_sync_cache_for(root: &Path, node: &str, cache: &SyncCache) {
    let path = cache_file_path(root, node);
    if let Ok(data) = serde_json::to_vec(cache) {
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&temporary, data).is_ok() {
            let _ = std::fs::rename(temporary, path);
        }
    }
}

pub(crate) static PROBED_NODES: std::sync::LazyLock<std::sync::Mutex<HashSet<(PathBuf, String)>>> =
    std::sync::LazyLock::new(Default::default);

/// Forgets the watermarks of `root` for every node.
pub fn clear_sync_cache(root: &Path) {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    PROBED_NODES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(r, _)| r != &canonical);
    crate::call_tree::clear_call_hierarchy_cache_for(root);
    for path in cache_file_paths(root) {
        let _ = std::fs::remove_file(path);
    }
}

/// Forgets the watermark of `root` for one node only.
pub fn clear_sync_cache_for(root: &Path, node: &str) {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    PROBED_NODES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(canonical, node.to_string()));
    crate::call_tree::clear_call_hierarchy_cache_for(root);
    let _ = std::fs::remove_file(cache_file_path(root, node));
}

/// Drops `rel_paths` from every node's watermark of `root`, so the next sync to any node
/// uploads them again. Used after the client rewrote files itself (refactorings), which no
/// gateway has seen.
pub fn forget_synced_files(root: &Path, rel_paths: &[String]) {
    crate::call_tree::clear_call_hierarchy_cache_for(root);
    for path in cache_file_paths(root) {
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let Ok(mut cache) = serde_json::from_slice::<SyncCache>(&data) else {
            continue;
        };
        let mut changed = false;
        for rel in rel_paths {
            changed |= cache.files.remove(rel).is_some();
        }
        if changed && let Ok(bytes) = serde_json::to_vec(&cache) {
            let _ = std::fs::write(&path, bytes);
        }
    }
}

/// Drops `rel_paths` from the watermark of `root` for gateway `node` and marks them to be sent
/// by the next sync to that node whatever git says about them: the gateway reported them stale,
/// gone from its copy of the workspace (#262).
pub fn resend_lost_files(root: &Path, node: &str, rel_paths: &[String]) {
    if rel_paths.is_empty() {
        return;
    }
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut cache = load_sync_cache_for(&canonical_root, node);
    for rel in rel_paths {
        let path = Path::new(rel);
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue; // never let the server name a file outside the checkout to upload
        }
        cache.files.remove(rel);
        cache.resend.insert(rel.clone());
    }
    save_sync_cache_for(&canonical_root, node, &cache);
}
