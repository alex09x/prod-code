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

pub async fn apply_sync(
    storage_root: &std::path::Path,
    workspace_manager: &WorkspaceManager,
    req: SyncRequest,
) -> SyncResponse {
    apply_sync_with_metrics(storage_root, workspace_manager, None, req).await
}

pub async fn apply_sync_with_metrics(
    storage_root: &std::path::Path,
    workspace_manager: &WorkspaceManager,
    metrics: Option<&metrics::Metrics>,
    req: SyncRequest,
) -> SyncResponse {
    let start = Instant::now();
    let server_workspace = workspace::resolve_server_workspace(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    // A directory nobody handshook or probed into is either brand new or was reset behind the
    // client's back; a delta landing there must not pass for a complete workspace.
    let workspace_was_fresh = !server_workspace.join(workspace::LAST_USED_MARKER).exists();
    if !workspace_was_fresh {
        workspace::touch_last_used(&server_workspace);
    }
    let folder_name = server_workspace
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("default");
    // A workspace that is already warm in RAM must see the synced files as its new base.
    let loaded_rust = workspace_manager
        .get_loaded(&server_workspace)
        .await
        .map(|ws| ws.mirrored_rust_engines())
        .unwrap_or_default();

    let mut files_updated = 0;
    let mut files_deleted = 0;
    let mut bytes_transferred = 0;

    let arrived: Vec<String> = req.files.iter().map(|f| f.relative_path.clone()).collect();
    let synced: Vec<(String, Option<u64>)> = req
        .files
        .iter()
        .map(|f| {
            (
                f.relative_path.clone(),
                f.content.as_deref().map(content_hash),
            )
        })
        .collect();
    let mut project_config_changed = false;
    let mut watched = Vec::new();
    // Files that could not be written: not recorded as synced, and reported stale so that the
    // client sends them again (#385).
    let mut failed: Vec<String> = Vec::new();
    for delta in req.files {
        let target_path = match safe_sync_target(&server_workspace, &delta.relative_path).await {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(error = %error, file = %delta.relative_path, "sync rejected unsafe path");
                failed.push(delta.relative_path);
                continue;
            }
        };
        project_config_changed |= is_project_config_file(&delta.relative_path);
        match delta.content {
            Some(content_bytes) => {
                bytes_transferred += content_bytes.len();
                let kind = if target_path.exists() {
                    workspace::WatchedChange::Changed
                } else {
                    workspace::WatchedChange::Created
                };
                if let Err(e) =
                    write_synced_file(&target_path, &content_bytes, delta.is_executable).await
                {
                    tracing::warn!(error = %e, file = %target_path.display(), "sync write failed; the client sends it again");
                    failed.push(delta.relative_path);
                    continue;
                }
                files_updated += 1;
                watched.push((target_path.clone(), kind));
                if let Ok(text) = std::str::from_utf8(&content_bytes) {
                    for engine_lock in &loaded_rust {
                        let mut engine = engine_lock.lock().await;
                        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            engine.update_base(&target_path, Some(text.to_string()))
                        }));
                        match res {
                            Ok(Err(e)) => {
                                tracing::warn!(error = %e, file = %target_path.display(), "base update failed");
                            }
                            Err(_) => {
                                tracing::warn!(file = %target_path.display(), "base update panicked; continuing");
                            }
                            Ok(Ok(())) => {}
                        }
                    }
                }
            }
            None => {
                if target_path.exists() && tokio::fs::remove_file(&target_path).await.is_ok() {
                    files_deleted += 1;
                    watched.push((target_path.clone(), workspace::WatchedChange::Deleted));
                    prune_empty_parents(&server_workspace, target_path.parent());
                }
                for engine_lock in &loaded_rust {
                    let mut engine = engine_lock.lock().await;
                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        engine.update_base(&target_path, None)
                    }));
                    match res {
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, file = %target_path.display(), "base removal failed");
                        }
                        Err(_) => {
                            tracing::warn!(file = %target_path.display(), "base removal panicked; continuing");
                        }
                        Ok(Ok(())) => {}
                    }
                }
            }
        }
    }
    let synced: Vec<(String, Option<u64>)> = synced
        .into_iter()
        .filter(|(path, _)| !failed.contains(path))
        .collect();
    workspace::record_synced(&server_workspace, &synced);
    workspace::touch_last_used(&server_workspace);
    let mut stale_paths =
        workspace::clear_stale_paths(&server_workspace, arrived.iter().map(String::as_str));
    workspace::record_stale_paths(&server_workspace, &failed);
    for path in failed {
        if !stale_paths.contains(&path) {
            stale_paths.push(path);
        }
    }
    // gopls does not watch the tree itself: without this it went on answering from the content
    // a file no session had open had when it first read it (#317).
    for loaded in workspace_manager.loaded_under(&server_workspace).await {
        loaded.notify_watched_files(&watched).await;
    }
    workspace_manager.editor_servers.notify(&watched).await;

    // A changed project manifest (tsconfig, package.json, pyproject, CMakeLists, Package.swift,
    // go.mod, Cargo.toml ...) changes what the language server should see: drop the loaded
    // engines so the next session starts them on the new configuration.
    if project_config_changed {
        let dropped = workspace_manager.unload_under(&server_workspace).await;
        if dropped > 0 {
            tracing::info!(
                folder_name,
                dropped,
                "project configuration changed; engines reloaded on next session"
            );
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    if files_updated > 0 || files_deleted > 0 || workspace_was_fresh {
        tracing::info!(
            folder_name,
            files_updated,
            files_deleted,
            bytes_transferred,
            fresh = workspace_was_fresh,
            duration_ms = %format!("{duration_ms}ms"),
            "[SYNC] Workspace fast-sync applied"
        );
    } else {
        tracing::debug!(
            folder_name,
            duration_ms = %format!("{duration_ms}ms"),
            "[SYNC] Workspace fast-sync (no changes)"
        );
    }

    if let Some(metrics) = metrics {
        let mut ev = metrics::Event::blank("sync");
        ev.workspace = folder_name.to_string();
        ev.items = (files_updated + files_deleted) as u64;
        ev.bytes = bytes_transferred as u64;
        ev.duration_ms = duration_ms;
        metrics.record(ev);
    }
    SyncResponse {
        files_updated,
        files_deleted,
        bytes_transferred,
        duration_ms,
        server_workspace_root: server_workspace.to_string_lossy().to_string(),
        workspace_was_fresh,
        stale_paths,
    }
}

