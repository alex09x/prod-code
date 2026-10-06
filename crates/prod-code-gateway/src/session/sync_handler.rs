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
use crate::*;
use std::path::PathBuf;
use std::time::Instant;

pub async fn handle_session_sync(
    req: SyncRequest,
    view: &SessionView,
    out_tx: &SharedOutputSender,
) {
    let start = Instant::now();
    let mut files_updated = 0;
    let mut files_deleted = 0;
    let mut bytes_transferred = 0;
    let mut watched = Vec::new();
    let mut failed: Vec<String> = Vec::new();

    for delta in &req.files {
        let target_path = view.workspace.root.join(&delta.relative_path);
        match &delta.content {
            Some(content_bytes) => {
                bytes_transferred += content_bytes.len();
                let kind = if target_path.exists() {
                    workspace::WatchedChange::Changed
                } else {
                    workspace::WatchedChange::Created
                };
                if let Err(e) =
                    write_synced_file(&target_path, content_bytes, delta.is_executable).await
                {
                    tracing::warn!(
                        error = %e,
                        file = %target_path.display(),
                        "sync write failed; the client sends it again"
                    );
                    failed.push(delta.relative_path.clone());
                    continue;
                }
                files_updated += 1;
                watched.push((target_path.clone(), kind));
                // The workspace is this worktree's own: synced files are its
                // new base, visible to every session except one that still
                // holds an unsaved buffer for the same path.
                if let Ok(text) = std::str::from_utf8(content_bytes) {
                    for engine_lock in view.workspace.mirrored_rust_engines() {
                        let mut engine = engine_lock.lock().await;
                        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            engine.update_base(&target_path, Some(text.to_string()))
                        }));
                        match res {
                            Ok(Err(e)) => {
                                tracing::warn!(
                                    error = %e,
                                    file = %target_path.display(),
                                    "base update failed"
                                );
                            }
                            Err(_) => {
                                tracing::warn!(
                                    file = %target_path.display(),
                                    "base update panicked; continuing"
                                );
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
                    prune_empty_parents(&view.workspace.root, target_path.parent());
                }
                for engine_lock in view.workspace.mirrored_rust_engines() {
                    let mut engine = engine_lock.lock().await;
                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        engine.update_base(&target_path, None)
                    }));
                    match res {
                        Ok(Err(e)) => {
                            tracing::warn!(
                                error = %e,
                                file = %target_path.display(),
                                "base removal failed"
                            );
                        }
                        Err(_) => {
                            tracing::warn!(
                                file = %target_path.display(),
                                "base removal panicked; continuing"
                            );
                        }
                        Ok(Ok(())) => {}
                    }
                }
            }
        }
    }

    if req.clean_others && let Some(engine_lock) = &view.workspace.rust_engine {
        // The request is the session's complete dirty set: any other
        // overlay this session still holds is stale (reverted or committed).
        let keep: Vec<PathBuf> = req
            .files
            .iter()
            .map(|delta| view.workspace.root.join(&delta.relative_path))
            .collect();
        let mut engine = engine_lock.lock().await;
        match engine.retain_session_overlays(view.session_id, &keep) {
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
        }
    }

    let mut stale_paths = workspace::clear_stale_paths(
        &view.workspace.root,
        req.files.iter().map(|delta| delta.relative_path.as_str()),
    );
    workspace::record_stale_paths(&view.workspace.root, &failed);
    for path in failed {
        if !stale_paths.contains(&path) {
            stale_paths.push(path);
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
}
