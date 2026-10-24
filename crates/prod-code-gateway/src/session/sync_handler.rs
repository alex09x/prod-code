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
    ) = tokio::spawn(async move {
        let sync_guard = sync_guard;
        let mut files_updated = 0;
        let mut files_deleted = 0;
        let mut bytes_transferred = 0;
        let mut watched = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let mut synced: Vec<(String, Option<u64>)> = Vec::with_capacity(req.files.len());
        let mut safe_paths = Vec::with_capacity(req.files.len());
        let mut base_paths = Vec::new();
        let arrived: Vec<String> = req.files.iter().map(|f| f.relative_path.clone()).collect();

        for delta in req.files {
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
                    let content_digest = content_hash(&content_bytes);
                    bytes_transferred += content_bytes.len();
                    let kind = if target_path.exists() {
                        workspace::WatchedChange::Changed
                    } else {
                        workspace::WatchedChange::Created
                    };
                    if let Err(e) = write_synced_file_for_workspace(
                        &workspace_root,
                        &target_path,
                        &content_bytes,
                        delta.is_executable,
                    )
                    .await
                    {
                        tracing::warn!(
                            error = %e,
                            file = %target_path.display(),
                            "sync write failed; the client sends it again"
                        );
                        failed.push(delta.relative_path);
                        continue;
                    }
                    files_updated += 1;
                    synced.push((delta.relative_path, Some(content_digest)));
                    watched.push((target_path.clone(), kind));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn session_sync_rejects_absolute_parent_and_symlink_paths() {
        use std::os::unix::fs::symlink;

        let storage = tempfile::tempdir().unwrap();
        let outside = storage.path();
        let root = storage.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        symlink(outside, root.join("linked")).unwrap();

        let victim = outside.join("victim.txt");
        std::fs::write(&victim, b"keep").unwrap();
        let absolute_write = outside.join("absolute-escape.txt");
        let parent_write = storage.path().join("parent-escape.txt");
        let symlink_write = outside.join("symlink-escape.txt");

        let workspace = Arc::new(workspace::SharedWorkspace::new(
            root.clone(),
            "generic-lsp".to_string(),
            None,
            None,
            None,
            None,
        ));
        let manager = workspace::WorkspaceManager::new();
        let lease = workspace::WorkspaceLease::acquire(workspace);
        let view = manager.register_session_view(1, root, lease).await;
        let (raw_tx, mut raw_rx) = rapidfire::mpsc::bounded(8);
        let out_tx = SharedOutputSender::new(raw_tx, std::time::Duration::from_secs(1));

        handle_session_sync(
            SyncRequest {
                client_workspace_root: "/tmp/ws".to_string(),
                files: vec![
                    FileDelta {
                        relative_path: "../parent-escape.txt".to_string(),
                        content: Some(b"parent".to_vec()),
                        is_executable: false,
                    },
                    FileDelta {
                        relative_path: absolute_write.to_string_lossy().into_owned(),
                        content: Some(b"absolute".to_vec()),
                        is_executable: false,
                    },
                    FileDelta {
                        relative_path: "linked/symlink-escape.txt".to_string(),
                        content: Some(b"symlink".to_vec()),
                        is_executable: false,
                    },
                    FileDelta {
                        relative_path: "../victim.txt".to_string(),
                        content: None,
                        is_executable: false,
                    },
                ],
                clean_others: false,
                base_workspace_name: Some("ws".to_string()),
            },
            &view,
            &out_tx,
        )
        .await;

        let response = raw_rx.recv().await.unwrap().message;
        let WireMessage::SyncResponse(response) = response else {
            panic!("expected sync response");
        };
        assert_eq!(response.files_updated, 0);
        assert_eq!(response.files_deleted, 0);
        for rejected in [
            "../parent-escape.txt",
            absolute_write.to_str().unwrap(),
            "linked/symlink-escape.txt",
            "../victim.txt",
        ] {
            assert!(
                response.stale_paths.iter().any(|path| path == rejected),
                "invalid path should be retried: {rejected:?}; stale paths: {:?}",
                response.stale_paths
            );
        }
        assert!(!parent_write.exists());
        assert!(!absolute_write.exists());
        assert!(!symlink_write.exists());
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
    }
}
