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
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::shared::SharedWorkspace;

pub struct SessionView {
    pub session_id: u64,
    pub worktree_root: PathBuf,
    /// The workspace the session's queries run against: the loaded one, or the validation view
    /// derived from it.
    pub workspace: Arc<SharedWorkspace>,
    /// The loaded workspace the session is counted against, for idle eviction.
    pub accounted: Arc<SharedWorkspace>,
    pub is_single_owner: Arc<AtomicBool>,
    pub direct_edit_open_files: Arc<std::sync::Mutex<HashMap<PathBuf, String>>>,
    pub(crate) lease: Option<WorkspaceLease>,
    pub(crate) owner: Option<WorktreeOwner>,
}

impl SessionView {
    pub fn is_single_owner(&self) -> bool {
        self.is_single_owner.load(Ordering::SeqCst)
    }

    pub(crate) fn start_retirement(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        let mut lease = self.lease.take()?;
        let owner = self.owner.take();
        let workspace = Arc::clone(&self.workspace);
        let session_id = self.session_id;
        let direct_edits: Vec<PathBuf> = self
            .direct_edit_open_files
            .lock()
            .map(|mut files| files.drain().map(|(p, _)| p).collect())
            .unwrap_or_default();
        Some(tokio::spawn(async move {
            clear_session_overlays(&workspace, session_id, &direct_edits).await;
            drop(owner);
            let base_lease = lease._base_lease.take();
            if let Some(ws) = lease.workspace.take() {
                ws.touch();
                let remaining = ws.active_sessions.fetch_sub(1, Ordering::SeqCst) - 1;
                if remaining == 0 && ws.unloaded.load(Ordering::SeqCst) {
                    ws.detach_overlay().await;
                }
            }
            drop(base_lease);
            drop(lease);
        }))
    }

    pub(crate) async fn retire(mut self) {
        if let Some(retirement) = self.start_retirement()
            && let Err(err) = retirement.await
        {
            tracing::warn!(%err, session_id = self.session_id, "session retirement task failed");
        }
    }
}

impl Drop for SessionView {
    fn drop(&mut self) {
        let _ = self.start_retirement();
    }
}

/// One counted attachment to a loaded workspace. Until it is transferred into a
/// [`SessionView`], dropping the handshake future returns the count automatically.
pub struct WorkspaceLease {
    pub(crate) workspace: Option<Arc<SharedWorkspace>>,
    pub(crate) _base_lease: Option<Box<WorkspaceLease>>,
}

impl WorkspaceLease {
    pub(crate) fn acquire(workspace: Arc<SharedWorkspace>) -> Self {
        workspace.active_sessions.fetch_add(1, Ordering::Relaxed);
        let base_lease = workspace
            .base_workspace
            .as_ref()
            .map(|base| Box::new(WorkspaceLease::acquire(Arc::clone(base))));
        Self {
            workspace: Some(workspace),
            _base_lease: base_lease,
        }
    }

    pub fn workspace(&self) -> &Arc<SharedWorkspace> {
        self.workspace.as_ref().expect("a live workspace lease")
    }
}

impl Deref for WorkspaceLease {
    type Target = SharedWorkspace;
    fn deref(&self) -> &Self::Target {
        self.workspace()
    }
}

impl Drop for WorkspaceLease {
    fn drop(&mut self) {
        let base_lease = self._base_lease.take();
        if let Some(workspace) = self.workspace.take() {
            workspace.touch();
            let remaining = workspace.active_sessions.fetch_sub(1, Ordering::SeqCst) - 1;
            if remaining == 0 && workspace.unloaded.load(Ordering::SeqCst) {
                let ws = Arc::clone(&workspace);
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        ws.detach_overlay().await;
                        drop(base_lease);
                    });
                    return;
                } else {
                    let _ = std::thread::spawn(move || {
                        if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                        {
                            rt.block_on(async move {
                                ws.detach_overlay().await;
                                drop(base_lease);
                            });
                        }
                    });
                    return;
                }
            }
        }
        drop(base_lease);
    }
}

#[derive(Clone)]
pub struct DirectEditLeaseHandle {
    pub session_id: u64,
    pub is_active: Arc<AtomicBool>,
    pub open_files: Arc<std::sync::Mutex<HashMap<PathBuf, String>>>,
    pub workspace: Arc<SharedWorkspace>,
}

#[derive(Default)]
pub struct WorktreeEntry {
    pub count: usize,
    pub direct_edit_lease: Option<DirectEditLeaseHandle>,
}

pub(crate) type WorktreeOwners = Arc<std::sync::Mutex<HashMap<PathBuf, WorktreeEntry>>>;

pub struct WorktreeOwner {
    pub(crate) root: PathBuf,
    pub(crate) session_id: u64,
    pub(crate) owners: WorktreeOwners,
}

impl Drop for WorktreeOwner {
    fn drop(&mut self) {
        let mut owners = self.owners.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = owners.get_mut(&self.root) {
            entry.count = entry.count.saturating_sub(1);
            if let Some(lease) = &entry.direct_edit_lease
                && lease.session_id == self.session_id
            {
                entry.direct_edit_lease = None;
            }
            if entry.count == 0 {
                owners.remove(&self.root);
            }
        }
    }
}

pub(crate) async fn clear_session_overlays(
    workspace: &SharedWorkspace,
    session_id: u64,
    direct_edits: &[PathBuf],
) {
    if let Some(engine_lock) = &workspace.rust_engine {
        let mut engine = engine_lock.lock().await;
        for path in direct_edits {
            if let Err(err) = engine.reload_file(path) {
                tracing::warn!(error = %err, file = %path.display(), session_id, "failed to reload direct-edit file on session retirement");
            }
        }
        if let Err(err) = engine.clear_session(session_id) {
            tracing::warn!(error = %err, session_id, "failed to drop session overlays");
        }
    }
    if let Some(engine) = &workspace.generic_engine
        && let Err(err) = engine.close_session(session_id).await
    {
        tracing::warn!(error = %err, session_id, "failed to drop generic session overlays");
    }
}

/// Migrates unsaved direct edits from a revoked single-owner session into its session overlay in the engine,
/// and restores the clean on-disk text into the base Salsa database.
pub async fn migrate_direct_edits_to_overlays(lease: DirectEditLeaseHandle) {
    if let Some(engine_lock) = &lease.workspace.rust_engine {
        let mut engine = engine_lock.lock().await;
        let files: Vec<(PathBuf, String)> = {
            let mut open = lease.open_files.lock().unwrap_or_else(|e| e.into_inner());
            open.drain().collect()
        };
        for (path, buffer_text) in files {
            if let Err(e) = engine.reload_file(&path) {
                tracing::warn!(error = %e, file = %path.display(), "failed to reload disk text before migrating direct edit");
            }
            if let Err(e) = engine.set_session_overlay(lease.session_id, &path, Some(buffer_text)) {
                tracing::warn!(error = %e, session = lease.session_id, file = %path.display(), "failed to migrate direct edit to session overlay");
            }
        }
    }
}
