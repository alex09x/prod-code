/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;


pub fn is_ram_cache_enabled_with(enabled: bool, env_val: Option<&str>) -> bool {
    enabled
        || env_val
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
}

pub fn is_ram_cache_enabled(enabled: bool) -> bool {
    let env = std::env::var("PROD_CODE_BUILD_RAM").ok();
    is_ram_cache_enabled_with(enabled, env.as_deref())
}

/// Resolves or initializes an isolated in-memory RAM-disk build cache for `workspace` (Roadmap 6.2).
/// Returns `Some(PathBuf)` if enabled and headroom permits (>= 20% free and >= 256 MiB free); otherwise `None`.
pub fn resolve_ram_build_cache(
    workspace: &Path,
    enabled: bool,
    custom_dir: Option<&Path>,
) -> Option<PathBuf> {
    if !is_ram_cache_enabled(enabled) {
        return None;
    }
    let default_shm = Path::new("/dev/shm/prod-code-build");
    let fallback_tmp = Path::new("/tmp/prod-code-build");
    let base_dir = custom_dir.unwrap_or_else(|| {
        if Path::new("/dev/shm").is_dir() {
            default_shm
        } else {
            fallback_tmp
        }
    });

    if let Some(space) = disk_space(base_dir) {
        let free_share = space.free as f64 / space.total.max(1) as f64;
        pub(crate) const MIN_FREE_RAM_SHARE: f64 = 0.20;
        pub(crate) const MIN_FREE_BYTES: u64 = 256 * 1024 * 1024;
        if free_share < MIN_FREE_RAM_SHARE || space.free < MIN_FREE_BYTES {
            tracing::info!(
                dir = %base_dir.display(),
                free_mb = space.free / (1024 * 1024),
                "🌱 [BUILD_RAM] insufficient RAM disk headroom; falling back to disk cache"
            );
            return None;
        }
    }

    let workspace_name = workspace.file_name()?.to_str()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&workspace, &mut hasher);
    let hash = std::hash::Hasher::finish(&hasher);
    let ws_cache_dir = base_dir.join(format!("{workspace_name}-{hash:016x}"));
    let target_dir = ws_cache_dir.join("target");
    if let Err(e) = std::fs::create_dir_all(&target_dir) {
        tracing::warn!(%e, dir = %target_dir.display(), "failed to create RAM build cache dir; falling back to disk");
        return None;
    }
    let marker = ws_cache_dir.join(".last_used");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&marker);
    Some(target_dir)
}

/// An RAII lease that marks a RAM-disk cache directory as actively in use by a running build.
///
/// On Unix, an advisory flock is held on the marker file for the entire lifetime of the lease.
/// If the gateway process is killed or crashes, the OS kernel automatically closes the file
/// descriptor and releases the lock, allowing sweepers to identify and clean stale markers.
pub struct RamBuildLease {
    pub marker: Option<PathBuf>,
    #[cfg(unix)]
    _lock_file: Option<std::fs::File>,
}

impl RamBuildLease {
    pub fn acquire(target_dir: &Path) -> Self {
        if let Some(ws_cache_dir) = target_dir.parent() {
            let lease_id = NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let marker = ws_cache_dir.join(format!(".active_{}_{}", pid, lease_id));
            #[cfg(unix)]
            {
                use std::io::Write;
                use std::os::unix::io::AsRawFd;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&marker)
                {
                    let ret =
                        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                    if ret == 0 {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let meta = format!(
                            "{{\"pid\":{},\"lease_id\":{},\"created_at\":{}}}\n",
                            pid, lease_id, now
                        );
                        let _ = file.write_all(meta.as_bytes());
                        let _ = file.flush();
                        return Self {
                            marker: Some(marker),
                            _lock_file: Some(file),
                        };
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let meta = format!(
                    "{{\"pid\":{},\"lease_id\":{},\"created_at\":{}}}\n",
                    pid, lease_id, now
                );
                if let Ok(()) = std::fs::write(&marker, meta.as_bytes()) {
                    return Self {
                        marker: Some(marker),
                    };
                }
            }
        }
        Self {
            marker: None,
            #[cfg(unix)]
            _lock_file: None,
        }
    }
}

impl Drop for RamBuildLease {
    fn drop(&mut self) {
        if let Some(ref path) = self.marker {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Checks whether a RAM-disk lease marker represents an active build process.
///
/// On Unix, an advisory flock is held for the lifetime of a live lease. If the process has died or crashed,
/// flock acquisition succeeds; this function unlinks the stale marker and returns `false`.
/// If the lock cannot be acquired because a running process is holding it, returns `true`.
pub fn is_ram_lease_active(marker: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(marker)
        {
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if ret == 0 {
                // Successfully locked: owning process died or exited without dropping the lease.
                // Remove the stale marker file while holding the lock.
                let _ = std::fs::remove_file(marker);
                false
            } else {
                // Lock busy: active build process holds this lease.
                true
            }
        } else {
            // Already unlinked or cannot open
            false
        }
    }
    #[cfg(not(unix))]
    {
        if let Ok(meta) = marker.metadata() {
            if let Ok(elapsed) = meta.modified().and_then(|m| m.elapsed()) {
                if elapsed.as_secs() > 7200 {
                    let _ = std::fs::remove_file(marker);
                    return false;
                }
            }
        }
        true
    }
}

/// Sweeps stale or orphaned RAM-disk build caches on startup or periodic maintenance.
pub fn sweep_ram_build_caches(base_dir: &Path) -> usize {
    if !base_dir.is_dir() {
        return 0;
    }
    let running: std::collections::HashSet<String> = RUNNING_COMMANDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .map(|(ws, _, _)| ws.clone())
        .collect();

    let mut removed = 0;
    if let Ok(entries) = std::fs::read_dir(base_dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let path = entry.path();
                let dir_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                let is_running = running.iter().any(|ws| dir_name.starts_with(ws));
                if is_running {
                    continue;
                }
                if let Ok(children) = std::fs::read_dir(&path) {
                    let mut has_active_lease = false;
                    for child in children.flatten() {
                        let name = child.file_name();
                        let name_str = name.to_str().unwrap_or_default();
                        if name_str.starts_with(".active_") {
                            if is_ram_lease_active(&child.path()) {
                                has_active_lease = true;
                            }
                        }
                    }
                    if has_active_lease {
                        continue;
                    }
                }
                let marker = path.join(".last_used");
                let metadata_target = if marker.is_file() {
                    marker.metadata().ok()
                } else {
                    entry.metadata().ok()
                };
                if let Some(meta) = metadata_target {
                    let is_old = meta
                        .modified()
                        .ok()
                        .and_then(|m| m.elapsed().ok())
                        .map(|age| age.as_secs() > 86400)
                        .unwrap_or(false);
                    if is_old {
                        if let Ok(()) = std::fs::remove_dir_all(&path) {
                            removed += 1;
                        }
                    }
                }
            }
        }
    }
    if removed > 0 {
        tracing::info!(dir = %base_dir.display(), removed, "swept old RAM-disk build caches");
    }
    removed
}

