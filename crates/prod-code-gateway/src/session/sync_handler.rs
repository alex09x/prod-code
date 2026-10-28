/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::shared_output::SharedOutputSender;
use crate::sync::file_write::write_synced_file_for_workspace;
use crate::*;
use std::time::Instant;

pub async fn handle_session_sync(
    req: SyncRequest,
    view: &SessionView,
    out_tx: &SharedOutputSender,
) {
    let start = Instant::now();
    let sync_guard = crate::sync::lock_workspace_sync(
        &view.workspace.root,
        "sync",
        crate::sync::SYNC_LOCK_REPORT_EVERY,
    )
    .await;
    let workspace_root = view.workspace.root.clone();
    let (
        _sync_guard,
        files_updated,
        files_deleted,
        bytes_transferred,
        watched,
        safe_paths,
        base_paths,
        stale_paths,
        project_config_changed,
    ) = tokio::spawn(async move {
        let sync_guard = sync_guard;
        let mut files_updated = 0;
        let mut files_deleted = 0;
        let mut bytes_transferred = 0;
        let mut watched = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let mut project_config_changed = false;
        let mut synced: Vec<(String, Option<u64>)> = Vec::with_capacity(req.files.len());
        let mut safe_paths = Vec::with_capacity(req.files.len());
        let mut base_paths = Vec::new();
        let arrived: Vec<String> = req.files.iter().map(|f| f.relative_path.clone()).collect();

        for delta in req.files {
            let is_project_config =
                crate::lsp::rename_helper::is_project_config_file(&delta.relative_path);
            let target_path = match safe_sync_target(&workspace_root, &delta.relative_path).await {
                Ok(path) => path,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        file = %delta.relative_path,
                        "session sync rejected unsafe path"
                    );
                    failed.push(delta.relative_path);
                    continue;
                }
            };
            safe_paths.push(target_path.clone());
            match delta.content {
                Some(content_bytes) => {
                    bytes_transferred += content_bytes.len();
                    let kind = if target_path.exists() {
                        workspace::WatchedChange::Changed
                    } else {
                        workspace::WatchedChange::Created
                    };
                    let committed_digest = match write_synced_file_for_workspace(
                        &workspace_root,
                        &target_path,
                        &content_bytes,
                        delta.is_executable,
                    )
                    .await
                    {
                        Ok(digest) => digest,
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                file = %target_path.display(),
                                "sync write failed; the client sends it again"
                            );
                            failed.push(delta.relative_path);
                            continue;
                        }
                    };
                    files_updated += 1;
                    synced.push((delta.relative_path, Some(committed_digest)));
                    watched.push((target_path.clone(), kind));
                    project_config_changed |= is_project_config;
                    // The workspace is this worktree's own: synced files are its
                    // new base, visible to every session except one that still
                    // holds an unsaved buffer for the same path.
                    base_paths.push(target_path);
                }
                None => {
                    if target_path.exists() {
                        match tokio::fs::remove_file(&target_path).await {
                            Ok(()) => {
                                let _ =
                                    crate::sync::config_meta::forget(&workspace_root, &target_path)
                                        .await;
                                files_deleted += 1;
                                project_config_changed |= is_project_config;
                                watched
                                    .push((target_path.clone(), workspace::WatchedChange::Deleted));
                                prune_empty_parents(&workspace_root, target_path.parent());
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
        crate::sync::test_hooks::post_write(&workspace_root).await;
        workspace::record_synced(&workspace_root, &synced);
        let mut stale_paths =
            workspace::clear_stale_paths(&workspace_root, arrived.iter().map(String::as_str));
        workspace::record_stale_paths(&workspace_root, &failed);
        for path in failed {
            if !stale_paths.contains(&path) {
                stale_paths.push(path);
            }
        }
        (
            sync_guard,
            files_updated,
            files_deleted,
            bytes_transferred,
            watched,
            safe_paths,
            base_paths,
            stale_paths,
            project_config_changed,
        )
    })
    .await
    .expect("session sync disk commit task must not panic");

    // A stuck query can keep the engine locked; the answer must not wait for it (#1143).
    crate::sync::base_updates::push_base_updates(
        view.workspace.mirrored_rust_engines(),
        base_paths,
        crate::sync::base_updates::BASE_UPDATE_WAIT,
    )
    .await;

    if req.clean_others
        && let Some(engine_lock) = &view.workspace.rust_engine
    {
        // The request is the session's complete dirty set: any other
        // overlay this session still holds is stale (reverted or committed).
        // A busy engine keeps them until the next full sync rather than stalling this one.
        match tokio::time::timeout(
            crate::sync::base_updates::BASE_UPDATE_WAIT,
            engine_lock.lock(),
        )
        .await
        {
            Ok(mut engine) => match engine.retain_session_overlays(view.session_id, &safe_paths) {
                Ok(dropped) if dropped > 0 => tracing::info!(
                    session = view.session_id,
                    dropped,
                    "🧹 [OVERLAY] dropped stale session buffers after full dirty sync"
                ),
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session = view.session_id,
                        "failed to drop stale session buffers"
                    )
                }
            },
            Err(_) => tracing::warn!(
                session = view.session_id,
                "Rust engine busy; stale session buffers are dropped on a later sync"
            ),
        }
    }

    view.workspace.notify_watched_files(&watched).await;
    let duration_ms = start.elapsed().as_millis() as u64;
    let _ = out_tx
        .send(WireMessage::SyncResponse(SyncResponse {
            files_updated,
            files_deleted,
            bytes_transferred,
            duration_ms,
            server_workspace_root: view.workspace.root.to_string_lossy().to_string(),
            workspace_was_fresh: false,
            stale_paths,
        }))
        .await;
    if project_config_changed {
        view.accounted.mark_unloaded();
    }
}

#[cfg(test)]
mod tests;
