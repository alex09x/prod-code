/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::loader::LoadState;
use super::super::session::{
    DirectEditLeaseHandle, SessionView, WorkspaceLease, WorktreeOwner,
    migrate_direct_edits_to_overlays,
};
use super::super::shared::SharedWorkspace;
use super::types::WorkspaceManager;

impl WorkspaceManager {
    /// Trigger rebalance redirect for all active sessions of a workspace matching `name`.
    pub async fn trigger_rebalance_by_name(
        &self,
        name: &str,
        target_addr: String,
        reason: Option<String>,
    ) -> usize {
        let base_name = name.split('#').next().unwrap_or(name).trim();
        let guard = self.workspaces.read().await;
        let mut notified = 0;
        for (key, state) in guard.iter() {
            if let LoadState::Ready(ws) = state {
                let ws_name = ws
                    .root
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default();
                let matches_exact_name = ws_name == name || ws_name == base_name;
                let matches_exact_path = ws.root == Path::new(name)
                    || ws.root == Path::new(base_name)
                    || key.0 == Path::new(name)
                    || key.0 == Path::new(base_name);
                let matches_worktree_base = ws.base_workspace.as_ref().is_some_and(|b| {
                    let b_name = b
                        .root
                        .file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default();
                    b_name == name
                        || b_name == base_name
                        || b.root == Path::new(name)
                        || b.root == Path::new(base_name)
                });

                if matches_exact_name || matches_exact_path || matches_worktree_base {
                    notified += ws.trigger_rebalance(target_addr.clone(), reason.clone());
                }
            }
        }
        notified
    }

    /// Loaded workspaces and their active session counts.
    pub async fn loaded_workspaces_for_rebalance(&self) -> Vec<(Arc<SharedWorkspace>, usize)> {
        let guard = self.workspaces.read().await;
        guard
            .values()
            .filter_map(|state| match state {
                LoadState::Ready(ws) => {
                    let active = ws.active_sessions.load(Ordering::Relaxed);
                    Some((Arc::clone(ws), active))
                }
                _ => None,
            })
            .collect()
    }

    /// Register a session's view over a worktree.
    pub async fn register_session_view(
        &self,
        session_id: u64,
        worktree_root: PathBuf,
        lease: WorkspaceLease,
    ) -> SessionView {
        let workspace = Arc::clone(lease.workspace());
        workspace.touch();

        let mut previous_lease = None;
        let is_single_owner = Arc::new(AtomicBool::new(false));
        let direct_edit_open_files = Arc::new(std::sync::Mutex::new(HashMap::new()));

        {
            let mut owners = self
                .worktree_owners
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let entry = owners.entry(worktree_root.clone()).or_default();
            entry.count += 1;
            if entry.count == 1 {
                is_single_owner.store(true, Ordering::SeqCst);
                entry.direct_edit_lease = Some(DirectEditLeaseHandle {
                    session_id,
                    is_active: Arc::clone(&is_single_owner),
                    open_files: Arc::clone(&direct_edit_open_files),
                    workspace: Arc::clone(&workspace),
                });
            } else if let Some(existing) = entry.direct_edit_lease.take() {
                existing.is_active.store(false, Ordering::SeqCst);
                previous_lease = Some(existing);
            }
        }

        if let Some(prev) = previous_lease {
            migrate_direct_edits_to_overlays(prev).await;
        }

        SessionView {
            session_id,
            worktree_root: worktree_root.clone(),
            accounted: Arc::clone(&workspace),
            workspace,
            is_single_owner,
            direct_edit_open_files,
            lease: Some(lease),
            owner: Some(WorktreeOwner {
                root: worktree_root,
                session_id,
                owners: Arc::clone(&self.worktree_owners),
            }),
        }
    }

    /// Restore overlays before releasing worktree and workspace ownership.
    pub async fn unregister_session_view(&self, view: SessionView) {
        view.retire().await;
    }

    #[doc(hidden)]
    pub fn worktree_owner_count_for_test(&self, root: &Path) -> usize {
        self.worktree_owners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(root)
            .map(|entry| entry.count)
            .unwrap_or(0)
    }
}
