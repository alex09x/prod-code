/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::manager::WorkspaceManager;
use super::types::unix_now;

/// Marker file whose mtime records the last handshake on a workspace directory.
pub const LAST_USED_MARKER: &str = ".prod-code-last-used";

/// How long a workspace directory has gone unused: since its last-used marker, or the directory
/// itself when it has none.
pub(crate) fn idle_for(path: &Path, now: SystemTime) -> Duration {
    std::fs::metadata(path.join(LAST_USED_MARKER))
        .or_else(|_| std::fs::metadata(path))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| now.duration_since(t).ok())
        .unwrap_or_default()
}

/// The share of the filesystem holding `path` that is free for use (0.0 to 1.0); `None` when it
/// cannot be read.
pub fn free_share(path: &Path) -> Option<f64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `statvfs` only writes the struct it is given, and the path is NUL-terminated.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
            return None;
        }
        let total = stat.f_blocks as f64 * stat.f_frsize as f64;
        (total > 0.0).then(|| stat.f_bavail as f64 * stat.f_frsize as f64 / total)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// The free and total bytes of the filesystem holding `path`; `None` when it cannot be read (#809, #810).
pub fn free_and_total_bytes(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `statvfs` only writes the struct it is given, and the path is NUL-terminated.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
            return None;
        }
        let total = stat.f_blocks as u64 * stat.f_frsize as u64;
        let free = stat.f_bavail as u64 * stat.f_frsize as u64;
        Some((free, total))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// How long a worktree copy must have gone unused before it may be removed to free space: one in
/// use between two sessions stays.
pub(crate) const SPACE_PRUNE_MIN_IDLE: Duration = Duration::from_secs(3600);

/// Removes idle `<repo>--wt-*` copies that are not loaded, oldest first, while the storage
/// filesystem has less than `min_free` (a share of its size) free, however young they are: they
/// are rebuildable, and a client whose worktree comes back resyncs. Forty-seven of them filled a
/// 913 GB disk in two days, well before any was seven days idle, and the full disk then truncated
/// synced files (#385, #386). `free` reads the free share. Returns the removed paths.
pub async fn prune_worktree_dirs_for_space(
    storage_root: &Path,
    min_free: f64,
    manager: &WorkspaceManager,
    free: impl Fn(&Path) -> Option<f64>,
) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Some(mut share) = free(storage_root) else {
        return removed;
    };
    if share >= min_free {
        return removed;
    }
    let Ok(entries) = std::fs::read_dir(storage_root) else {
        return removed;
    };
    let now = SystemTime::now();
    let mut candidates: Vec<(Duration, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !path.is_dir() || !name.contains("--wt-") || manager.is_loaded(&path).await {
            continue;
        }
        let idle = idle_for(&path, now);
        if idle >= SPACE_PRUNE_MIN_IDLE {
            candidates.push((idle, path));
        }
    }
    candidates.sort_by_key(|(idle, _)| std::cmp::Reverse(*idle));
    for (idle, path) in candidates {
        if share >= min_free {
            break;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                let before = share;
                share = free(storage_root).unwrap_or(share);
                tracing::info!(
                    workspace = %path.display(),
                    idle_hours = idle.as_secs() / 3600,
                    free_before = %format!("{:.1}%", before * 100.0),
                    free_after = %format!("{:.1}%", share * 100.0),
                    "🧹 pruned a worktree workspace to free disk space"
                );
                removed.push(path);
            }
            Err(e) => {
                tracing::warn!(error = %e, workspace = %path.display(), "failed to prune worktree workspace")
            }
        }
    }
    if share < min_free {
        tracing::warn!(
            free = %format!("{:.1}%", share * 100.0),
            wanted = %format!("{:.1}%", min_free * 100.0),
            "storage is still low on space after pruning idle worktree copies"
        );
    }
    removed
}

/// Removes `<repo>--wt-*` workspace directories that have not been used for `max_age` and are
/// not loaded. A client whose worktree reappears simply resyncs. Returns the removed paths.
pub async fn prune_stale_worktree_dirs(
    storage_root: &Path,
    max_age: Duration,
    manager: &WorkspaceManager,
) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(storage_root) else {
        return removed;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !path.is_dir() || !name.contains("--wt-") {
            continue;
        }
        if manager.is_loaded(&path).await {
            continue;
        }
        let idle = idle_for(&path, now);
        if idle < max_age {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                crate::sync::config_meta::remove_workspace(&path);
                tracing::info!(
                    workspace = %path.display(),
                    idle_hours = idle.as_secs() / 3600,
                    idle_secs = idle.as_secs(),
                    "🧹 pruned stale worktree workspace"
                );
                removed.push(path);
            }
            Err(e) => {
                tracing::warn!(error = %e, workspace = %path.display(), "failed to prune worktree workspace")
            }
        }
    }
    removed
}

/// Removes main (non-worktree) workspace directories that have not been used for `max_age`,
/// are not loaded, and have no active or loaded worktree copies. Returns the removed paths.
pub async fn prune_stale_main_workspace_dirs(
    storage_root: &Path,
    max_age: Duration,
    worktree_max_age: Duration,
    manager: &WorkspaceManager,
) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(storage_root) else {
        return removed;
    };
    let now = SystemTime::now();
    let all_paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    crate::sync::config_meta::prune_orphans(storage_root);

    for path in &all_paths {
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip hidden directories (e.g. .prod-code-shadow, .git)
        if name.starts_with('.') || name == "lost+found" {
            continue;
        }
        // Worktrees are handled separately by prune_stale_worktree_dirs
        if name.contains("--wt-") {
            continue;
        }
        // Main workspace must not be loaded in memory
        if manager.is_loaded(path).await {
            continue;
        }
        let idle = idle_for(path, now);
        if idle < max_age {
            continue;
        }

        // Base repository protection:
        // Check if any worktree of this base repo is currently loaded or still active on disk.
        let wt_prefix = format!("{name}--wt-");
        let mut has_active_worktree = false;
        for other in &all_paths {
            let Some(other_name) = other.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if other.is_dir() && other_name.starts_with(&wt_prefix) {
                // If the worktree is loaded in memory, the base repo must not be pruned
                if manager.is_loaded(other).await {
                    has_active_worktree = true;
                    break;
                }
                // If worktree pruning is disabled (worktree_max_age == 0) or worktree was used within worktree_max_age,
                // the worktree is still active on disk, so protect the base repo.
                if worktree_max_age.is_zero() || idle_for(other, now) < worktree_max_age {
                    has_active_worktree = true;
                    break;
                }
            }
        }

        if has_active_worktree {
            tracing::debug!(
                workspace = %path.display(),
                "main workspace is idle but protected by active worktrees"
            );
            continue;
        }

        match std::fs::remove_dir_all(path) {
            Ok(()) => {
                crate::sync::config_meta::remove_workspace(path);
                tracing::info!(
                    workspace = %path.display(),
                    idle_hours = idle.as_secs() / 3600,
                    idle_secs = idle.as_secs(),
                    "🧹 pruned stale main workspace"
                );
                removed.push(path.clone());
            }
            Err(e) => {
                tracing::warn!(error = %e, workspace = %path.display(), "failed to prune main workspace")
            }
        }
    }
    removed
}

/// Records a handshake on the workspace directory for [`prune_stale_worktree_dirs`].
pub fn touch_last_used(workspace_dir: &Path) {
    let marker = workspace_dir.join(LAST_USED_MARKER);
    let _ = std::fs::write(&marker, unix_now().to_string());
}

/// Persists a specific timestamp as the last-used time on the workspace directory.
pub fn touch_last_used_at(workspace_dir: &Path, ts: u64) {
    let marker = workspace_dir.join(LAST_USED_MARKER);
    let _ = std::fs::write(&marker, ts.to_string());
    if let Ok(file) = std::fs::File::options().write(true).open(&marker) {
        let system_time = SystemTime::UNIX_EPOCH + Duration::from_secs(ts);
        let _ = file.set_modified(system_time);
    }
}
