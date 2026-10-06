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
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::super::loader::{LoadState, release};
use super::super::prune::touch_last_used_at;
use super::super::shared::SharedWorkspace;
use super::super::types::{WorkspaceKey, unix_now};
use super::types::{RECLAIM_MIN_IDLE, WorkspaceManager};

impl WorkspaceManager {
    /// Drops every loaded workspace that has had no session for `idle` (engines and their
    /// databases are freed once the last reference goes). Returns the evicted roots.
    pub async fn evict_idle(&self, idle: Duration) -> Vec<PathBuf> {
        let now = unix_now();
        let evicted = self
            .remove_idle(|candidates| {
                candidates
                    .into_iter()
                    .filter(|ws| {
                        now.saturating_sub(ws.last_used.load(Ordering::Relaxed)) >= idle.as_secs()
                    })
                    .collect()
            })
            .await;
        for ws in &evicted {
            let ts = ws.last_used.load(Ordering::Relaxed);
            touch_last_used_at(&ws.root, ts);
        }
        let roots = evicted.iter().map(|ws| ws.root.clone()).collect();
        release(evicted).await;
        roots
    }

    /// Takes the workspaces `pick` chooses among those without a session out of the map, under
    /// its lock, and hands them back to be dropped after it is released.
    async fn remove_idle(
        &self,
        pick: impl FnOnce(Vec<Arc<SharedWorkspace>>) -> Vec<Arc<SharedWorkspace>>,
    ) -> Vec<Arc<SharedWorkspace>> {
        let mut guard = self.workspaces.write().await;
        let idle = guard
            .values()
            .filter_map(|state| match state {
                // Sessions are counted under this lock's read side, so none can attach while
                // the count is read here. A base workspace with attached worktrees cannot be
                // evicted until all attached worktrees have been evicted.
                LoadState::Ready(ws)
                    if ws.active_sessions.load(Ordering::Relaxed) == 0
                        && ws.attached_worktrees.load(Ordering::Relaxed) == 0 =>
                {
                    Some(Arc::clone(ws))
                }
                _ => None,
            })
            .collect();
        let picked = pick(idle);
        for ws in &picked {
            guard.remove(&ws.key);
        }
        picked
    }

    /// Unloads idle engines, least recently used first, until what they are counted to free
    /// covers `excess` bytes. Only engines without a session for [`RECLAIM_MIN_IDLE`] are
    /// taken, never `loading`. Returns how many were unloaded.
    async fn reclaim_idle(&self, excess: u64, loading: &WorkspaceKey) -> usize {
        let now = unix_now();
        let admission = Arc::clone(&self.admission);
        let reclaimed = self
            .remove_idle(|candidates| {
                let mut candidates: Vec<_> = candidates
                    .into_iter()
                    .filter(|ws| {
                        ws.key != *loading
                            && now.saturating_sub(ws.last_used.load(Ordering::Relaxed))
                                >= RECLAIM_MIN_IDLE.as_secs()
                    })
                    .collect();
                candidates.sort_by_key(|ws| ws.last_used.load(Ordering::Relaxed));
                let mut freed = 0;
                candidates
                    .into_iter()
                    .take_while(|ws| {
                        let take = freed < excess;
                        freed = freed.saturating_add(ws.reclaimable(&admission));
                        take
                    })
                    .collect()
            })
            .await;
        for ws in &reclaimed {
            tracing::info!(workspace = %ws.root.display(), engine = %ws.engine, excess_bytes = excess, "💤 [RECLAIM] unloaded an idle engine to make room for a new one");
        }
        let count = reclaimed.len();
        release(reclaimed).await;
        count
    }

    /// Admits a new engine of `engine` for `loading`: at once when the host has room for it,
    /// otherwise after unloading idle engines, if that makes room. The host is read again once
    /// they are gone, so only memory actually returned counts.
    pub(crate) async fn admit(
        &self,
        engine: &str,
        loading: &WorkspaceKey,
    ) -> Result<crate::admission::Reservation, crate::admission::CapacityRefused> {
        let shortfall = match self.admission.try_reserve(engine) {
            Ok(reservation) => return Ok(reservation),
            Err(shortfall) => shortfall,
        };
        let reclaimed = self.reclaim_idle(shortfall.excess(), loading).await;
        if reclaimed == 0 {
            return Err(crate::admission::CapacityRefused {
                shortfall,
                reclaimed,
            });
        }
        self.admission
            .try_reserve(engine)
            .map_err(|shortfall| crate::admission::CapacityRefused {
                shortfall,
                reclaimed,
            })
    }

    /// Drops every loaded workspace rooted at or below `prefix` (the checkout and the engines
    /// of its nested projects), so the next session loads it afresh. Sessions that still hold
    /// the old workspace keep it until they end. Returns how many were dropped.
    pub async fn unload_under(&self, prefix: &Path) -> usize {
        let mut unloaded = Vec::new();
        let count = {
            let mut guard = self.workspaces.write().await;
            let keys: Vec<WorkspaceKey> = guard
                .keys()
                .filter(|key| key.0.starts_with(prefix))
                .cloned()
                .collect();
            for key in &keys {
                if let Some(LoadState::Ready(ws)) = guard.remove(key) {
                    unloaded.push(ws);
                }
            }
            keys.len()
        };
        release(unloaded).await;
        count
    }

    /// The loaded workspaces rooted at or below `prefix`: the checkout's and those of its
    /// nested projects.
    pub async fn loaded_under(&self, prefix: &Path) -> Vec<Arc<SharedWorkspace>> {
        let guard = self.workspaces.read().await;
        guard
            .iter()
            .filter_map(|(key, state)| match state {
                LoadState::Ready(ws) if key.0.starts_with(prefix) => Some(Arc::clone(ws)),
                _ => None,
            })
            .collect()
    }

    /// Whether a workspace is currently loaded (or loading) at `workspace_root`.
    pub async fn is_loaded(&self, workspace_root: &Path) -> bool {
        self.workspaces
            .read()
            .await
            .contains_key(&WorkspaceKey(workspace_root.to_path_buf()))
    }

    #[doc(hidden)]
    pub async fn insert_ready_for_test(&self, workspace: Arc<SharedWorkspace>) {
        let mut guard = self.workspaces.write().await;
        guard.insert(workspace.key.clone(), LoadState::Ready(workspace));
    }

    /// The already loaded workspace at `workspace_root`, if any.
    pub async fn get_loaded(&self, workspace_root: &Path) -> Option<Arc<SharedWorkspace>> {
        let guard = self.workspaces.read().await;
        match guard.get(&WorkspaceKey(workspace_root.to_path_buf())) {
            Some(LoadState::Ready(ws)) => Some(Arc::clone(ws)),
            _ => None,
        }
    }

    /// The loaded workspaces: name (directory name), engine and active sessions.
    pub async fn loaded_summary(&self) -> Vec<(String, String, usize)> {
        let guard = self.workspaces.read().await;
        guard
            .values()
            .filter_map(|state| match state {
                LoadState::Ready(ws) => Some((
                    ws.root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    ws.engine.clone(),
                    ws.active_sessions.load(Ordering::Relaxed),
                )),
                _ => None,
            })
            .collect()
    }

    /// Number of currently loaded workspaces.
    pub async fn loaded_count(&self) -> usize {
        let guard = self.workspaces.read().await;
        guard
            .values()
            .filter(|state| matches!(state, LoadState::Ready(_)))
            .count()
    }
}
