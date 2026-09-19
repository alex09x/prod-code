//! Workspace management: multi-tenant shared workspaces, leader-follower coalescing, and worktree views.

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Mutex, RwLock, broadcast};

/// Unique identifier for a shared workspace based on its canonical root.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceKey(pub PathBuf);

/// A loaded base workspace shared across multiple sessions/worktrees.
pub struct SharedWorkspace {
    pub key: WorkspaceKey,
    pub root: PathBuf,
    pub engine: String,
    pub active_sessions: AtomicUsize,
    pub direct_edit_eligible: AtomicBool,
    pub rust_engine: Option<Arc<Mutex<prod_code_engine_rust::RustEngine>>>,
    pub go_engine: Option<Arc<prod_code_engine_go::GoEngine>>,
    pub generic_engine: Option<Arc<prod_code_engine_generic::GenericLspEngine>>,
    pub backend: Option<Arc<crate::backend::BackendWorker>>,
}

impl SharedWorkspace {
    pub fn new(
        root: PathBuf,
        engine: String,
        rust_engine: Option<Arc<Mutex<prod_code_engine_rust::RustEngine>>>,
        go_engine: Option<Arc<prod_code_engine_go::GoEngine>>,
        generic_engine: Option<Arc<prod_code_engine_generic::GenericLspEngine>>,
        backend: Option<Arc<crate::backend::BackendWorker>>,
    ) -> Self {
        Self {
            key: WorkspaceKey(root.clone()),
            root,
            engine,
            active_sessions: AtomicUsize::new(0),
            direct_edit_eligible: AtomicBool::new(true),
            rust_engine,
            go_engine,
            generic_engine,
            backend,
        }
    }
}

/// A session's private view over a shared workspace (e.g. an agent's Git worktree).
pub struct SessionView {
    pub session_id: u64,
    pub worktree_root: PathBuf,
    pub workspace: Arc<SharedWorkspace>,
    pub is_single_owner: bool,
}

/// State of an in-flight workspace load.
enum LoadState {
    Loading(broadcast::Sender<Result<Arc<SharedWorkspace>, String>>),
    Ready(Arc<SharedWorkspace>),
}

/// Thread-safe manager coordinating workspace lifecycle and leader-follower loading.
pub struct WorkspaceManager {
    workspaces: RwLock<HashMap<WorkspaceKey, LoadState>>,
    worktree_owners: Mutex<HashMap<PathBuf, usize>>,
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceManager {
    pub fn new() -> Self {
        Self {
            workspaces: RwLock::new(HashMap::new()),
            worktree_owners: Mutex::new(HashMap::new()),
        }
    }

    /// Number of currently loaded workspaces.
    pub async fn loaded_count(&self) -> usize {
        let guard = self.workspaces.read().await;
        guard
            .values()
            .filter(|state| matches!(state, LoadState::Ready(_)))
            .count()
    }

    /// Retrieve or load a shared workspace using leader-follower coalescing.
    ///
    /// If another session is already loading this workspace, current session becomes
    /// a follower and awaits the leader's result without duplicating compiler work.
    pub async fn get_or_load(
        &self,
        workspace_root: &Path,
        engine: &str,
    ) -> Result<Arc<SharedWorkspace>> {
        let key = WorkspaceKey(workspace_root.to_path_buf());

        // Fast path: check if already loaded
        {
            let guard = self.workspaces.read().await;
            if let Some(state) = guard.get(&key) {
                match state {
                    LoadState::Ready(ws) => {
                        ws.active_sessions.fetch_add(1, Ordering::Relaxed);
                        return Ok(Arc::clone(ws));
                    }
                    LoadState::Loading(tx) => {
                        let mut rx = tx.subscribe();
                        drop(guard);
                        return match rx.recv().await {
                            Ok(Ok(ws)) => {
                                ws.active_sessions.fetch_add(1, Ordering::Relaxed);
                                Ok(ws)
                            }
                            Ok(Err(err)) => anyhow::bail!("Workspace load failed: {err}"),
                            Err(e) => anyhow::bail!("Leader dropped load broadcast: {e}"),
                        };
                    }
                }
            }
        }

        // Slow path: acquire write lock to become leader
        let (tx, _rx) = broadcast::channel(1);
        {
            let mut guard = self.workspaces.write().await;
            // Double check
            if let Some(state) = guard.get(&key) {
                match state {
                    LoadState::Ready(ws) => {
                        ws.active_sessions.fetch_add(1, Ordering::Relaxed);
                        return Ok(Arc::clone(ws));
                    }
                    LoadState::Loading(existing_tx) => {
                        let mut sub = existing_tx.subscribe();
                        drop(guard);
                        return match sub.recv().await {
                            Ok(Ok(ws)) => {
                                ws.active_sessions.fetch_add(1, Ordering::Relaxed);
                                Ok(ws)
                            }
                            Ok(Err(err)) => anyhow::bail!("Workspace load failed: {err}"),
                            Err(e) => anyhow::bail!("Leader dropped load broadcast: {e}"),
                        };
                    }
                }
            }

            guard.insert(key.clone(), LoadState::Loading(tx.clone()));
        }

        // Leader performs actual workspace load
        tracing::info!(workspace = ?workspace_root, engine, "Leader starting workspace load");
        let mut rust_engine = None;
        let mut go_engine = None;
        let mut generic_engine = None;
        let mut backend = None;

        match engine {
            "rust" => {
                let ws_path = workspace_root.to_path_buf();
                let loaded_engine = tokio::task::spawn_blocking(move || {
                    prod_code_engine_rust::RustEngine::load(&ws_path)
                })
                .await
                .ok()
                .and_then(|res| match res {
                    Ok(e) => {
                        tracing::info!(workspace = ?workspace_root, "In-memory RustEngine (ra_ap_ide) loaded into RAM");
                        Some(Arc::new(Mutex::new(e)))
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "Failed to load in-memory RustEngine; falling back to subprocess");
                        None
                    }
                });

                if let Some(re) = loaded_engine {
                    rust_engine = Some(re);
                } else {
                    backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                        .await
                        .ok()
                        .map(Arc::new);
                }
            }
            "go" => {
                match prod_code_engine_go::GoEngine::load(
                    workspace_root,
                    prod_code_engine_go::GoConfig::default(),
                )
                .await
                {
                    Ok(ge) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GoEngine (gopls) active");
                        go_engine = Some(Arc::new(ge));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn GoEngine; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "python" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_python(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Python) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Python LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "typescript" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_typescript(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (TypeScript) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn TypeScript LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            _ => {
                backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                    .await
                    .ok()
                    .map(Arc::new);
            }
        }

        let ws = Arc::new(SharedWorkspace::new(
            workspace_root.to_path_buf(),
            engine.to_string(),
            rust_engine,
            go_engine,
            generic_engine,
            backend,
        ));
        ws.active_sessions.fetch_add(1, Ordering::Relaxed);

        // Transition state to Ready
        {
            let mut guard = self.workspaces.write().await;
            guard.insert(key, LoadState::Ready(Arc::clone(&ws)));
        }

        // Notify followers
        let _ = tx.send(Ok(Arc::clone(&ws)));
        Ok(ws)
    }

    /// Register a session's view over a worktree.
    ///
    /// Determines whether the session is the sole owner of this worktree path
    /// to activate the single-owner direct-edit fast path.
    pub async fn register_session_view(
        &self,
        session_id: u64,
        worktree_root: PathBuf,
        workspace: Arc<SharedWorkspace>,
    ) -> SessionView {
        let mut owners = self.worktree_owners.lock().await;
        let count = owners.entry(worktree_root.clone()).or_insert(0);
        *count += 1;
        let is_single_owner = *count == 1;

        SessionView {
            session_id,
            worktree_root,
            workspace,
            is_single_owner,
        }
    }

    /// Release a session's view on disconnect.
    pub async fn unregister_session_view(&self, view: &SessionView) {
        view.workspace
            .active_sessions
            .fetch_sub(1, Ordering::Relaxed);
        let mut owners = self.worktree_owners.lock().await;
        if let Some(count) = owners.get_mut(&view.worktree_root) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                owners.remove(&view.worktree_root);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_leader_follower_coalescing() {
        let manager = Arc::new(WorkspaceManager::new());
        let root = PathBuf::from("/test/workspace");

        // Concurrent requests for the same workspace
        let m1 = Arc::clone(&manager);
        let r1 = root.clone();
        let handle1 = tokio::spawn(async move { m1.get_or_load(&r1, "rust").await.unwrap() });

        let m2 = Arc::clone(&manager);
        let r2 = root.clone();
        let handle2 = tokio::spawn(async move { m2.get_or_load(&r2, "rust").await.unwrap() });

        let (ws1, ws2) = tokio::join!(handle1, handle2);
        let ws1 = ws1.unwrap();
        let ws2 = ws2.unwrap();

        // Both sessions share the exact same Arc instance in memory!
        assert!(Arc::ptr_eq(&ws1, &ws2));
        assert_eq!(manager.loaded_count().await, 1);
        assert_eq!(ws1.active_sessions.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn test_single_owner_detection() {
        let manager = WorkspaceManager::new();
        let root = PathBuf::from("/test/repo");
        let ws = manager.get_or_load(&root, "rust").await.unwrap();

        let wt1 = PathBuf::from("/test/repo/worktree-1");
        let wt2 = PathBuf::from("/test/repo/worktree-2");

        let view1 = manager
            .register_session_view(1, wt1.clone(), Arc::clone(&ws))
            .await;
        assert!(view1.is_single_owner, "First agent on wt1 is sole owner");

        let view2 = manager
            .register_session_view(2, wt2.clone(), Arc::clone(&ws))
            .await;
        assert!(view2.is_single_owner, "First agent on wt2 is sole owner");

        // Second session attaches to wt1
        let view3 = manager
            .register_session_view(3, wt1.clone(), Arc::clone(&ws))
            .await;
        assert!(
            !view3.is_single_owner,
            "Second agent on wt1 is NOT sole owner"
        );

        // Cleanup
        manager.unregister_session_view(&view1).await;
        manager.unregister_session_view(&view2).await;
        manager.unregister_session_view(&view3).await;
    }
}
