/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::file_write::write_synced_file_for_workspace;
use crate::*;

static WORKSPACE_SYNC_LOCKS: std::sync::LazyLock<
    std::sync::Mutex<
        std::collections::HashMap<std::path::PathBuf, std::sync::Arc<tokio::sync::Mutex<()>>>,
    >,
> = std::sync::LazyLock::new(Default::default);

pub(crate) fn server_workspace_sync_lock(
    server_workspace: &std::path::Path,
) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    WORKSPACE_SYNC_LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(server_workspace.to_path_buf())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// How often a sync or probe that waits for its workspace's sync lock says so in the log.
pub(crate) const SYNC_LOCK_REPORT_EVERY: Duration = Duration::from_secs(10);

/// Takes `server_workspace`'s sync lock for `what` (a sync or a probe), logging every
/// `report_every` while another one holds it. A wait that never ends was invisible: the
/// client timed out, and the gateway logged nothing for that workspace (#1143, #1155).
pub(crate) async fn lock_workspace_sync(
    server_workspace: &std::path::Path,
    what: &'static str,
    report_every: Duration,
) -> tokio::sync::OwnedMutexGuard<()> {
    let started = Instant::now();
    let mut acquire = Box::pin(server_workspace_sync_lock(server_workspace).lock_owned());
    loop {
        match tokio::time::timeout(report_every, &mut acquire).await {
            Ok(guard) => {
                if started.elapsed() >= report_every {
                    tracing::warn!(
                        workspace = %server_workspace.display(),
                        what,
                        waited_ms = started.elapsed().as_millis() as u64,
                        "took the workspace sync lock after a long wait"
                    );
                }
                return guard;
            }
            Err(_) => tracing::warn!(
                workspace = %server_workspace.display(),
                what,
                waited_secs = started.elapsed().as_secs(),
                "still waiting for the workspace sync lock: an earlier sync or probe of this workspace has not finished"
            ),
        }
    }
}

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
    let target = workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let sync_guard = lock_workspace_sync(&target, "sync", SYNC_LOCK_REPORT_EVERY).await;
    let storage_root_buf = storage_root.to_path_buf();
    let (
        _sync_guard,
        server_workspace,
        workspace_was_fresh,
        files_updated,
        files_deleted,
        bytes_transferred,
        project_config_changed,
        watched,
        base_paths,
        stale_paths,
    ) = tokio::spawn(async move {
        let sync_guard = sync_guard;
        let server_workspace = workspace::resolve_server_workspace(
            &storage_root_buf,
            &req.client_workspace_root,
            req.base_workspace_name.as_deref(),
        );
        // A directory nobody handshook or probed into is either brand new or was reset behind the
        // client's back; a delta landing there must not pass for a complete workspace.
        let workspace_was_fresh = !server_workspace.join(workspace::LAST_USED_MARKER).exists();
        if !workspace_was_fresh {
            workspace::touch_last_used(&server_workspace);
        }
        let mut files_updated = 0;
        let mut files_deleted = 0;
        let mut bytes_transferred = 0;
        let arrived: Vec<String> = req.files.iter().map(|f| f.relative_path.clone()).collect();
        let active_exec = workspace::active_exec_snapshot(&server_workspace);
        let synced_during_exec = active_exec
            .as_ref()
            .map(|(started, _)| workspace::synced_since(&server_workspace, *started))
            .unwrap_or_default();
        let mut project_config_changed = false;
        let mut watched = Vec::new();
        // Files that could not be written or deleted: not recorded as synced, and reported stale
        // so that the client sends them again (#385, #1251).
        let mut failed: Vec<String> = Vec::new();
        let mut synced: Vec<(String, Option<u64>)> = Vec::with_capacity(req.files.len());
        let mut base_paths = Vec::new();
        for delta in req.files {
            let target_path = match safe_sync_target(&server_workspace, &delta.relative_path).await
            {
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
                    let content_digest = content_hash(&content_bytes);
                    let incoming_stamp = (content_bytes.len() as u64, content_digest);
                    if target_path.exists()
                        && active_exec.as_ref().is_some_and(|(_, before)| {
                            before.get(delta.relative_path.as_str()) == Some(&incoming_stamp)
                        })
                        && !synced_during_exec.contains_key(delta.relative_path.as_str())
                    {
                        continue;
                    }
                    bytes_transferred += content_bytes.len();
                    let kind = if target_path.exists() {
                        workspace::WatchedChange::Changed
                    } else {
                        workspace::WatchedChange::Created
                    };
                    let committed_digest = match write_synced_file_for_workspace(
                        &server_workspace,
                        &target_path,
                        &content_bytes,
                        delta.is_executable,
                    )
                    .await
                    {
                        Ok(digest) => digest,
                        Err(e) => {
                            tracing::warn!(error = %e, file = %target_path.display(), "sync write failed; the client sends it again");
                            failed.push(delta.relative_path);
                            continue;
                        }
                    };
                    files_updated += 1;
                    synced.push((delta.relative_path, Some(committed_digest)));
                    watched.push((target_path.clone(), kind));
                    base_paths.push(target_path);
                }
                None => {
                    if target_path.exists() {
                        match tokio::fs::remove_file(&target_path).await {
                            Ok(()) => {
                                let _ = crate::sync::config_meta::forget(
                                    &server_workspace,
                                    &target_path,
                                )
                                .await;
                                files_deleted += 1;
                                watched
                                    .push((target_path.clone(), workspace::WatchedChange::Deleted));
                                prune_empty_parents(&server_workspace, target_path.parent());
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    file = %target_path.display(),
                                    "sync delete failed; the client sends it again"
                                );
                                failed.push(delta.relative_path);
                                continue;
                            }
                        }
                    }
                    synced.push((delta.relative_path, None));
                    base_paths.push(target_path);
                }
            }
        }
        #[cfg(test)]
        crate::sync::test_hooks::post_write(&server_workspace).await;
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
        (
            sync_guard,
            server_workspace,
            workspace_was_fresh,
            files_updated,
            files_deleted,
            bytes_transferred,
            project_config_changed,
            watched,
            base_paths,
            stale_paths,
        )
    })
    .await
    .expect("sync disk commit task must not panic");

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
    // The files are on disk: the answer does not wait on an engine that a stuck query keeps
    // locked (#1143).
    crate::sync::base_updates::push_base_updates(
        loaded_rust,
        base_paths,
        crate::sync::base_updates::BASE_UPDATE_WAIT,
    )
    .await;
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
        ev.method = "sync".to_string();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_unseeded_probe_defers_last_used_marker_until_upload() {
        let storage = tempfile::tempdir().unwrap();
        let wm = WorkspaceManager::new();
        let probe_resp = crate::sync::apply_sync_probe(
            storage.path(),
            &wm,
            SyncProbeRequest {
                client_workspace_root: "/tmp/client-go-repo".to_string(),
                base_workspace_name: Some("client-go-repo".to_string()),
                seed_from: None,
                files: vec![FileStamp {
                    relative_path: "go.mod".to_string(),
                    size: 24,
                    hash: 123,
                }],
            },
        )
        .await;
        assert_eq!(probe_resp.missing, vec!["go.mod".to_string()]);
        let ws_dir = std::path::PathBuf::from(&probe_resp.server_workspace_root);
        assert!(
            !ws_dir.join(workspace::LAST_USED_MARKER).exists(),
            "fresh unseeded workspace with missing files must not be marked ready before upload (#1098)"
        );

        let sync_resp = apply_sync(
            storage.path(),
            &wm,
            SyncRequest {
                client_workspace_root: "/tmp/client-go-repo".to_string(),
                base_workspace_name: Some("client-go-repo".to_string()),
                clean_others: false,
                files: vec![FileDelta {
                    relative_path: "go.mod".to_string(),
                    content: Some(b"module example.com/app\n".to_vec()),
                    is_executable: false,
                }],
            },
        )
        .await;
        assert!(sync_resp.workspace_was_fresh);
        assert!(ws_dir.join(workspace::LAST_USED_MARKER).exists());
        assert!(ws_dir.join("go.mod").is_file());
    }

    #[tokio::test]
    async fn a_reported_sync_lock_wait_keeps_waiting_and_takes_the_lock_once_free() {
        let workspace = std::path::PathBuf::from("/synthetic/sync-lock-wait");
        let held = server_workspace_sync_lock(&workspace).lock_owned().await;
        let waiter = tokio::spawn({
            let workspace = workspace.clone();
            async move {
                let _guard =
                    lock_workspace_sync(&workspace, "probe", Duration::from_millis(20)).await;
            }
        });
        // Several report intervals pass: the waiter logs, but neither gives up nor gets in.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!waiter.is_finished());
        drop(held);
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("the waiter takes the lock once it is released")
            .unwrap();
    }
}
