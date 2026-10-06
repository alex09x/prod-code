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

/// Writes a file a client synced so that a failed write, such as on a full disk, leaves the old
/// content: the text goes to a temporary file next to it, which then replaces it. `fs::write`
/// truncated the file first, and a full disk left it empty (#385).
pub async fn safe_sync_target(server_workspace: &Path, relative: &str) -> std::io::Result<PathBuf> {
    let invalid_path = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsafe sync path: {relative:?}"),
        )
    };
    let relative_path = Path::new(relative);
    if relative.is_empty() || relative.contains('\\') || relative_path.is_absolute() {
        return Err(invalid_path());
    }
    let components: Vec<&str> = relative.split('/').collect();
    if components
        .iter()
        .any(|component| component.is_empty() || *component == "." || *component == "..")
        || relative_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(invalid_path());
    }

    let mut current = server_workspace.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        current.push(*component);
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => {
                if metadata.file_type().is_symlink()
                    || (index + 1 < components.len() && !metadata.is_dir())
                {
                    return Err(invalid_path());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }

    Ok(server_workspace.join(relative_path))
}

pub async fn write_synced_file(
    target: &std::path::Path,
    content: &[u8],
    executable: bool,
) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = target.with_file_name(format!(
        ".{name}.prod-code-sync-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let written = async {
        tokio::fs::write(&temp, content).await?;
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755)).await?;
        }
        tokio::fs::rename(&temp, target).await
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    written
}

/// Removes `dir` and every parent left empty by that, up to but not including `root`: a directory
/// whose last file was deleted or moved away locally goes away on the copy too (#124). A
/// directory that still holds anything stops the climb.
pub fn prune_empty_parents(root: &std::path::Path, dir: Option<&std::path::Path>) {
    let mut dir = dir;
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(d).is_err() {
            return;
        }
        dir = d.parent();
    }
}

/// Removes every directory under `dir` that holds no file, however deeply, except the per-node
/// caches: what an earlier deletion left behind before empty directories were pruned (#124).
/// Returns whether `dir` itself is now empty.
pub fn prune_empty_dirs(root: &std::path::Path, dir: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut empty = true;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() && !path.is_symlink() && !is_node_cache(&name) {
            if prune_empty_dirs(root, &path) {
                let _ = std::fs::remove_dir(&path);
            } else {
                empty = false;
            }
        } else {
            empty = false;
        }
    }
    empty && dir != root
}

/// Compares the workspace directory with the client's manifest: deletes files the client does
/// not have, and the directories that leaves empty, and returns `(missing, deleted)` where
/// `missing` are manifest paths the server lacks or holds with different content.
pub fn reconcile_manifest(
    root: &std::path::Path,
    stamps: &[FileStamp],
) -> (Vec<String>, Vec<String>) {
    let wanted: std::collections::HashMap<&str, &FileStamp> = stamps
        .iter()
        .map(|s| (s.relative_path.as_str(), s))
        .collect();
    let mut present = Vec::new();
    walk_files(root, root, &mut present);
    let mut missing = Vec::new();
    let mut deleted = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (rel, path) in &present {
        match wanted.get(rel.as_str()) {
            None => {
                if std::fs::remove_file(path).is_ok() {
                    deleted.push(rel.clone());
                }
            }
            Some(stamp) => {
                seen.insert(stamp.relative_path.as_str());
                let same = std::fs::metadata(path)
                    .map(|m| m.len() == stamp.size)
                    .unwrap_or(false)
                    && std::fs::read(path)
                        .map(|bytes| content_hash(&bytes) == stamp.hash)
                        .unwrap_or(false);
                if !same {
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
    prune_empty_dirs(root, root);
    missing.sort();
    missing.dedup();
    (missing, deleted)
}

/// Answers a manifest probe: seeds a fresh workspace from the origin repository's copy,
/// reconciles it with the client's manifest, and reports what the client must still upload.
pub async fn apply_sync_probe(
    storage_root: &std::path::Path,
    workspace_manager: &WorkspaceManager,
    req: SyncProbeRequest,
) -> SyncProbeResponse {
    let start = Instant::now();
    let target = workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let fresh = !target.exists()
        || std::fs::read_dir(&target)
            .map(|mut d| d.next().is_none())
            .unwrap_or(true);
    let mut seeded = false;
    if fresh && let Some(seed) = req.seed_from.as_deref() {
        let seed_dir = storage_root.join(workspace::sanitize_identifier(seed.trim()));
        if seed_dir.is_dir() && seed_dir != target {
            let (from, to) = (seed_dir.clone(), target.clone());
            match tokio::task::spawn_blocking(move || {
                let files = copy_tree(&from, &to)?;
                let started = Instant::now();
                let cache = seed_build_cache(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding the build cache failed");
                    None
                });
                let cache_took = started.elapsed();
                let started = Instant::now();
                let packages = seed_dependency_trees(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding node_modules and virtual environments failed");
                    None
                });
                let packages_took = started.elapsed();
                let started = Instant::now();
                let cpp_cache = cpp_index::seed_cpp_worktree(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding C/C++ clangd index and compilation database failed");
                    None
                });
                let cpp_took = started.elapsed();
                let started = Instant::now();
                let swift_cache = swift_cache::seed_swift_worktree(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding Swift module cache and package checkouts failed");
                    None
                });
                let swift_took = started.elapsed();
                let started = Instant::now();
                let python_cache = python_cache::seed_python_worktree(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding Python virtual-environment stub cache failed");
                    None
                });
                let python_took = started.elapsed();
                let started = Instant::now();
                let ts_cache = ts_cache::seed_typescript_worktree(&from, &to).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "seeding TypeScript type declaration cache and configuration failed");
                    None
                });
                let ts_took = started.elapsed();
                Ok::<_, std::io::Error>((
                    files,
                    cache,
                    cache_took,
                    packages,
                    packages_took,
                    cpp_cache,
                    cpp_took,
                    swift_cache,
                    swift_took,
                    python_cache,
                    python_took,
                    ts_cache,
                    ts_took,
                ))
            })
            .await
            {
                Ok(Ok((
                    files,
                    cache,
                    cache_took,
                    packages,
                    packages_took,
                    cpp_cache,
                    cpp_took,
                    swift_cache,
                    swift_took,
                    python_cache,
                    python_took,
                    ts_cache,
                    ts_took,
                ))) => {
                    seeded = true;
                    tracing::info!(
                        workspace = %target.display(),
                        seed = %seed_dir.display(),
                        files,
                        build_cache_mb = cache.map(|bytes| bytes / (1024 * 1024)),
                        build_cache_ms = cache_took.as_millis() as u64,
                        dependencies_mb = packages.map(|bytes| bytes / (1024 * 1024)),
                        dependencies_ms = packages_took.as_millis() as u64,
                        cpp_cache_mb = cpp_cache.map(|bytes| bytes / (1024 * 1024)),
                        cpp_cache_ms = cpp_took.as_millis() as u64,
                        swift_cache_mb = swift_cache.map(|bytes| bytes / (1024 * 1024)),
                        swift_cache_ms = swift_took.as_millis() as u64,
                        python_cache_kb = python_cache.map(|bytes| bytes / 1024),
                        python_cache_ms = python_took.as_millis() as u64,
                        ts_cache_kb = ts_cache.map(|bytes| bytes / 1024),
                        ts_cache_ms = ts_took.as_millis() as u64,
                        "🌱 [SEED] new worktree workspace seeded from origin copy"
                    );
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, seed = %seed_dir.display(), "seeding failed")
                }
                Err(e) => tracing::warn!(error = %e, "seeding task failed"),
            }
        }
    }
    let _ = std::fs::create_dir_all(&target);
    workspace::touch_last_used(&target);

    let root = target.clone();
    let stamps = req.files;
    let manifest_len = stamps.len();
    let (missing, deleted) = tokio::task::spawn_blocking(move || {
        let reconciled = reconcile_manifest(&root, &stamps);
        workspace::forget_stale_paths(&root);
        reconciled
    })
    .await
    .unwrap_or_default();

    if !deleted.is_empty()
        && let Some(ws) = workspace_manager.get_loaded(&target).await
    {
        for engine_lock in ws.mirrored_rust_engines() {
            let mut engine = engine_lock.lock().await;
            for rel in &deleted {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    engine.update_base(&target.join(rel), None)
                }));
            }
        }
    }

    tracing::info!(
        workspace = %target.display(),
        manifest = manifest_len,
        missing = missing.len(),
        deleted = deleted.len(),
        seeded,
        duration_ms = %format!("{}ms", start.elapsed().as_millis()),
        "🔎 [PROBE] manifest reconciled"
    );

    SyncProbeResponse {
        server_workspace_root: target.to_string_lossy().to_string(),
        seeded,
        files_deleted: deleted.len(),
        missing,
    }
}
