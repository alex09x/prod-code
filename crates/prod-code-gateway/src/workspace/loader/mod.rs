/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(crate) mod backend;
pub(crate) mod generic;
pub(crate) mod go;
pub(crate) mod rust;

use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::broadcast;

use super::manager::WorkspaceManager;
use super::session::WorkspaceLease;
use super::shared::SharedWorkspace;
use super::types::WorkspaceKey;

/// State of an in-flight workspace load.
pub(crate) enum LoadState {
    Loading(broadcast::Sender<Result<Arc<SharedWorkspace>, String>>),
    Ready(Arc<SharedWorkspace>),
}

/// Drops unloaded workspaces on a blocking thread, after the map's lock is released: freeing an
/// analysis database takes a while, and must hold up neither other sessions nor a runtime
/// worker. Engines a session still holds live on until it ends.
pub(crate) async fn release(workspaces: Vec<Arc<SharedWorkspace>>) {
    if workspaces.is_empty() {
        return;
    }
    for ws in &workspaces {
        ws.unloaded.store(true, Ordering::SeqCst);
        if ws.active_sessions.load(Ordering::SeqCst) == 0 {
            ws.detach_overlay().await;
        }
    }
    let _ = tokio::task::spawn_blocking(move || drop(workspaces)).await;
}

/// Leads the load of `key` to its end: makes it Ready, or takes its Loading entry out of
/// the map when it failed or was refused, then answers the leader, if it still waits, and
/// the followers. Everything that could panic, dropping the replaced engines among it, runs
/// in a task of its own, so that a load that panics is answered too and never leaves its
/// Loading entry behind.
pub(crate) async fn lead(
    manager: Arc<WorkspaceManager>,
    key: WorkspaceKey,
    engine: String,
    tx: broadcast::Sender<Result<Arc<SharedWorkspace>, String>>,
    replaced: Vec<Arc<SharedWorkspace>>,
    leader: tokio::sync::oneshot::Sender<Result<WorkspaceLease>>,
) {
    let mgr = Arc::clone(&manager);
    let root = key.0.clone();
    let eng = engine.clone();
    let loading = tokio::spawn(async move {
        release(replaced).await;
        load_as_leader(&mgr, &root, &eng).await
    });
    let loaded = loading
        .await
        .unwrap_or_else(|err| Err(anyhow::anyhow!("the workspace load panicked: {err}")));
    match loaded {
        Ok((ws, reservation)) => {
            reservation.release_after_settling();
            let mut guard = manager.workspaces.write().await;
            let can_install = match guard.get(&key) {
                Some(LoadState::Loading(t)) => t.same_channel(&tx),
                None => true,
                _ => false,
            };
            if can_install {
                guard.insert(key, LoadState::Ready(Arc::clone(&ws)));
            }
            drop(guard);
            let session = (!leader.is_closed()).then(|| WorkspaceLease::acquire(Arc::clone(&ws)));
            if let Some(session) = session {
                let _ = leader.send(Ok(session));
            }
            let _ = tx.send(Ok(ws));
        }
        Err(err) => {
            {
                let mut guard = manager.workspaces.write().await;
                if matches!(guard.get(&key), Some(LoadState::Loading(t)) if t.same_channel(&tx)) {
                    guard.remove(&key);
                }
            }
            let _ = tx.send(Err(format!("{err:#}")));
            let _ = leader.send(Err(err));
        }
    }
}

/// Admits a new engine of `engine` at `workspace_root` and loads it. The reservation is
/// handed back with the workspace, to be held while the engine settles.
pub(crate) async fn load_as_leader(
    manager: &Arc<WorkspaceManager>,
    workspace_root: &Path,
    engine: &str,
) -> Result<(Arc<SharedWorkspace>, crate::admission::Reservation)> {
    let reservation = manager
        .admit(engine, &WorkspaceKey(workspace_root.to_path_buf()))
        .await
        .map_err(|refused| {
            tracing::warn!(workspace = ?workspace_root, engine, %refused, "🚫 [CAPACITY] refused to load a new engine");
            anyhow::Error::new(refused)
        })?;

    let _load_permit = manager
        .load_semaphore
        .acquire()
        .await
        .map_err(|e| anyhow::anyhow!("engine load semaphore closed: {e}"))?;

    tracing::info!(workspace = ?workspace_root, engine, "Leader starting workspace load");

    let (rust_engine, go_engine, generic_engine, backend) = if engine == "rust" {
        let (re, be) = rust::load_rust(workspace_root, &manager.rust_loader).await?;
        (re, None, None, be)
    } else if engine == "go" {
        let (ge, be) = go::load_go(workspace_root).await;
        (None, ge, None, be)
    } else if let Some((ge, be)) = generic::load_generic(workspace_root, engine).await {
        (None, None, ge, be)
    } else {
        let be = backend::load_backend_fallback(workspace_root, engine).await;
        (None, None, None, be)
    };

    let mut workspace = SharedWorkspace::new(
        workspace_root.to_path_buf(),
        engine.to_string(),
        rust_engine,
        go_engine,
        generic_engine,
        backend,
    );
    workspace.engine_load_semaphore = Arc::clone(&manager.load_semaphore);
    let ws = Arc::new(workspace);
    Ok((ws, reservation))
}
