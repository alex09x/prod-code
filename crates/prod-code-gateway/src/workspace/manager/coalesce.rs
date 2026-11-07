/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::broadcast;

use super::super::loader::{LoadState, lead, release};
use super::super::paths::split_worktree_base;
use super::super::session::WorkspaceLease;
use super::super::shared::SharedWorkspace;
use super::super::types::WorkspaceKey;
use super::types::WorkspaceManager;

#[cfg(test)]
mod tests;

impl WorkspaceManager {
    /// Retrieve or load a shared workspace using leader-follower coalescing, and note that it
    /// is in use, so that a restart can load it again (#1976).
    pub async fn get_or_load(
        self: &Arc<Self>,
        workspace_root: &Path,
        engine: &str,
    ) -> Result<WorkspaceLease> {
        let lease = self.get_or_load_untracked(workspace_root, engine).await?;
        if let Some(recent) = self.recent.get() {
            recent.touch(workspace_root, engine);
        }
        Ok(lease)
    }

    /// Loads `engine` for `workspace_root` ahead of any request, and only if nothing is there.
    /// A workspace is kept once per root, whatever its engine, and a load of another engine
    /// replaces the one that is there, so a load nobody asked for must not: a root that is
    /// loading, or loaded with a server that is alive, is left as it is, whichever engine it
    /// has. A load nobody asked for is also no use of the workspace, so it is not noted in the
    /// record of recent ones. A worktree is left alone as well: it attaches to its base's engine,
    /// and loading that could replace what the base holds (#1976).
    pub(crate) async fn load_ahead(
        self: &Arc<Self>,
        workspace_root: &Path,
        engine: &str,
    ) -> Result<crate::warmup::Ahead> {
        use crate::warmup::Ahead;
        if split_worktree_base(workspace_root).is_some_and(|base| base != workspace_root) {
            return Ok(Ahead::Worktree);
        }
        let key = WorkspaceKey(workspace_root.to_path_buf());
        let (tx, replaced) = {
            let mut guard = self.workspaces.write().await;
            match guard.get(&key) {
                Some(LoadState::Loading(_)) => return Ok(Ahead::Present),
                Some(LoadState::Ready(ws)) if !ws.has_dead_server() => return Ok(Ahead::Present),
                _ => {}
            }
            // Nothing there, or a workspace whose server has exited: take the slot, as a load
            // asked for by a request does.
            let mut replaced = Vec::new();
            if let Some(LoadState::Ready(ws)) = guard.remove(&key) {
                replaced.push(ws);
            }
            let (tx, _rx) = broadcast::channel(1);
            guard.insert(key.clone(), LoadState::Loading(tx.clone()));
            (tx, replaced)
        };
        let (leader_tx, leader_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(lead(
            Arc::clone(self),
            key,
            engine.to_string(),
            tx,
            replaced,
            leader_tx,
        ));
        match leader_rx.await {
            Ok(result) => result.map(|_lease| Ahead::Loaded),
            Err(_) => anyhow::bail!("the workspace load ended without a result"),
        }
    }

    /// [`Self::get_or_load`] without noting a use.
    pub(crate) async fn get_or_load_untracked(
        self: &Arc<Self>,
        workspace_root: &Path,
        engine: &str,
    ) -> Result<WorkspaceLease> {
        if engine == "rust"
            && let Some(base_root) = split_worktree_base(workspace_root)
            && base_root != workspace_root
        {
            return self.get_or_load_worktree(workspace_root, &base_root).await;
        }

        self.get_or_load_direct(workspace_root, engine).await
    }

    pub(crate) async fn get_or_load_direct(
        self: &Arc<Self>,
        workspace_root: &Path,
        engine: &str,
    ) -> Result<WorkspaceLease> {
        let key = WorkspaceKey(workspace_root.to_path_buf());

        loop {
            let waiting = {
                let guard = self.workspaces.read().await;
                match guard.get(&key) {
                    Some(LoadState::Ready(ws)) if ws.reusable_for(engine) => {
                        return Ok(WorkspaceLease::acquire(Arc::clone(ws)));
                    }
                    Some(LoadState::Ready(ws)) => {
                        tracing::info!(workspace = ?workspace_root, previous = %ws.engine, engine,
                            server_exited = ws.has_dead_server(),
                            "Workspace engine changed or its server exited; reloading");
                        None
                    }
                    Some(LoadState::Loading(tx)) => Some(tx.subscribe()),
                    None => None,
                }
            };
            if let Some(mut waiting) = waiting {
                match waiting.recv().await {
                    Ok(Ok(_)) => continue,
                    Ok(Err(err)) => anyhow::bail!("Workspace load failed: {err}"),
                    Err(err) => anyhow::bail!("Leader dropped load broadcast: {err}"),
                }
            }

            let (tx, _rx) = broadcast::channel(1);
            let mut replaced = Vec::new();
            let waiting = {
                let mut guard = self.workspaces.write().await;
                let stale = matches!(guard.get(&key), Some(LoadState::Ready(ws)) if !ws.reusable_for(engine));
                if stale && let Some(LoadState::Ready(ws)) = guard.remove(&key) {
                    replaced.push(ws);
                }
                match guard.get(&key) {
                    Some(LoadState::Ready(ws)) => {
                        return Ok(WorkspaceLease::acquire(Arc::clone(ws)));
                    }
                    Some(LoadState::Loading(existing)) => Some(existing.subscribe()),
                    None => {
                        guard.insert(key.clone(), LoadState::Loading(tx.clone()));
                        None
                    }
                }
            };
            if let Some(mut waiting) = waiting {
                match waiting.recv().await {
                    Ok(Ok(_)) => continue,
                    Ok(Err(err)) => anyhow::bail!("Workspace load failed: {err}"),
                    Err(err) => anyhow::bail!("Leader dropped load broadcast: {err}"),
                }
            }

            let (leader_tx, leader_rx) = tokio::sync::oneshot::channel();
            tokio::spawn(lead(
                Arc::clone(self),
                key.clone(),
                engine.to_string(),
                tx,
                replaced,
                leader_tx,
            ));
            return match leader_rx.await {
                Ok(result) => result,
                Err(_) => anyhow::bail!("the workspace load ended without a result"),
            };
        }
    }

    /// Retrieve or load a worktree workspace by attaching to the warm in-memory base engine.
    pub(crate) async fn get_or_load_worktree(
        self: &Arc<Self>,
        workspace_root: &Path,
        base_root: &Path,
    ) -> Result<WorkspaceLease> {
        let key = WorkspaceKey(workspace_root.to_path_buf());

        loop {
            let waiting = {
                let guard = self.workspaces.read().await;
                match guard.get(&key) {
                    Some(LoadState::Ready(ws)) if ws.reusable_for("rust") => {
                        return Ok(WorkspaceLease::acquire(Arc::clone(ws)));
                    }
                    Some(LoadState::Ready(_)) => None,
                    Some(LoadState::Loading(tx)) => Some(tx.subscribe()),
                    None => None,
                }
            };
            if let Some(mut waiting) = waiting {
                match waiting.recv().await {
                    Ok(Ok(_)) => continue,
                    Ok(Err(err)) => anyhow::bail!("Workspace load failed: {err}"),
                    Err(err) => anyhow::bail!("Leader dropped load broadcast: {err}"),
                }
            }

            let (tx, _rx) = broadcast::channel(1);
            let mut replaced = Vec::new();
            let waiting = {
                let mut guard = self.workspaces.write().await;
                let stale = matches!(guard.get(&key), Some(LoadState::Ready(ws)) if !ws.reusable_for("rust"));
                if stale && let Some(LoadState::Ready(ws)) = guard.remove(&key) {
                    replaced.push(ws);
                }
                match guard.get(&key) {
                    Some(LoadState::Ready(ws)) => {
                        return Ok(WorkspaceLease::acquire(Arc::clone(ws)));
                    }
                    Some(LoadState::Loading(existing)) => Some(existing.subscribe()),
                    None => {
                        guard.insert(key.clone(), LoadState::Loading(tx.clone()));
                        None
                    }
                }
            };
            if let Some(mut waiting) = waiting {
                match waiting.recv().await {
                    Ok(Ok(_)) => continue,
                    Ok(Err(err)) => anyhow::bail!("Workspace load failed: {err}"),
                    Err(err) => anyhow::bail!("Leader dropped load broadcast: {err}"),
                }
            }

            release(replaced).await;

            let base_lease = match self.get_or_load_direct(base_root, "rust").await {
                Ok(lease) => lease,
                Err(err) => {
                    let mut guard = self.workspaces.write().await;
                    guard.remove(&key);
                    let _ = tx.send(Err(format!("Base workspace load failed: {err:#}")));
                    return Err(err);
                }
            };

            let base_ws = Arc::clone(base_lease.workspace());
            let Some(base_engine_arc) = &base_ws.rust_engine else {
                let (leader_tx, leader_rx) = tokio::sync::oneshot::channel();
                tokio::spawn(lead(
                    Arc::clone(self),
                    key.clone(),
                    "rust".to_string(),
                    tx,
                    Vec::new(),
                    leader_tx,
                ));
                return match leader_rx.await {
                    Ok(result) => result,
                    Err(_) => anyhow::bail!("the workspace load ended without a result"),
                };
            };

            let copy_root = workspace_root.to_path_buf();
            let mirrored = base_ws.mirrored_rust_engines();
            let mut attached_engines = Vec::new();
            let mut attach_err = None;
            for eng_arc in &mirrored {
                let mut eng = eng_arc.lock().await;
                if let Err(e) = eng.attach_worktree(&copy_root) {
                    attach_err = Some(e);
                    break;
                }
                attached_engines.push(Arc::clone(eng_arc));
            }

            if let Some(err) = attach_err {
                for eng_arc in attached_engines {
                    let mut eng = eng_arc.lock().await;
                    eng.detach_worktree(&copy_root);
                }
                let mut guard = self.workspaces.write().await;
                guard.remove(&key);
                let msg = format!("Failed to attach worktree: {err:#}");
                let _ = tx.send(Err(msg.clone()));
                anyhow::bail!("{msg}");
            }

            let ws = Arc::new(SharedWorkspace::with_base(
                workspace_root.to_path_buf(),
                "rust".to_string(),
                Some(Arc::clone(base_engine_arc)),
                None,
                None,
                None,
                Some(Arc::clone(&base_ws)),
            ));

            base_ws.attached_worktrees.fetch_add(1, Ordering::Relaxed);

            let lease = WorkspaceLease::acquire(Arc::clone(&ws));
            {
                let mut guard = self.workspaces.write().await;
                guard.insert(key, LoadState::Ready(Arc::clone(&ws)));
            }
            let _ = tx.send(Ok(ws));
            return Ok(lease);
        }
    }
}
