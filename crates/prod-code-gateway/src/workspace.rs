//! Workspace management: multi-tenant shared workspaces, leader-follower coalescing, and worktree views.

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
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
    /// Unix seconds of the last session registration or retirement, for idle eviction.
    pub last_used: AtomicU64,
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
            last_used: AtomicU64::new(unix_now()),
            direct_edit_eligible: AtomicBool::new(true),
            rust_engine,
            go_engine,
            generic_engine,
            backend,
        }
    }
}

impl SharedWorkspace {
    pub fn touch(&self) {
        self.last_used.store(unix_now(), Ordering::Relaxed);
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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

    /// Drops every loaded workspace that has had no session for `idle` (engines and their
    /// databases are freed once the last reference goes). Returns the evicted roots.
    pub async fn evict_idle(&self, idle: Duration) -> Vec<PathBuf> {
        let now = unix_now();
        let mut guard = self.workspaces.write().await;
        let stale: Vec<WorkspaceKey> = guard
            .iter()
            .filter_map(|(key, state)| match state {
                LoadState::Ready(ws)
                    if ws.active_sessions.load(Ordering::Relaxed) == 0
                        && now.saturating_sub(ws.last_used.load(Ordering::Relaxed))
                            >= idle.as_secs() =>
                {
                    Some(key.clone())
                }
                _ => None,
            })
            .collect();
        let mut evicted = Vec::with_capacity(stale.len());
        for key in stale {
            guard.remove(&key);
            evicted.push(key.0);
        }
        evicted
    }

    /// Whether a workspace is currently loaded (or loading) at `workspace_root`.
    pub async fn is_loaded(&self, workspace_root: &Path) -> bool {
        self.workspaces
            .read()
            .await
            .contains_key(&WorkspaceKey(workspace_root.to_path_buf()))
    }

    #[cfg(test)]
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
                    LoadState::Ready(ws) if ws.engine != engine => {
                        // The directory was (re)populated since this workspace was loaded,
                        // e.g. an empty worktree workspace detected as generic before its
                        // first sync landed. Fall through and load it with the right engine.
                        tracing::info!(
                            workspace = ?workspace_root,
                            previous = %ws.engine,
                            engine,
                            "Workspace engine kind changed; reloading"
                        );
                    }
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
            let stale =
                matches!(guard.get(&key), Some(LoadState::Ready(ws)) if ws.engine != engine);
            if stale {
                guard.remove(&key);
            }
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
        workspace.touch();
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
        view.workspace.touch();
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

/// Extract a clean, generic workspace identifier from any client workspace or worktree path.
/// Short stable hash of a client root, used to give every worktree its own server workspace.
pub fn worktree_suffix(client_root: &str) -> String {
    let hash = client_root
        .bytes()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        }) as u32;
    format!("--wt-{hash:08x}")
}

pub fn extract_workspace_identifier(client_root: &str) -> String {
    let path = Path::new(client_root);

    // Normalize path components
    let components: Vec<&str> = path
        .iter()
        .filter_map(|c| c.to_str())
        .filter(|&c| c != "/" && c != "\\" && !c.is_empty())
        .collect();

    // 1. Check for standard runner worktree container:
    // e.g. ".../worktrees/<workspace_name>/task-<id>/..."
    for (i, &seg) in components.iter().enumerate() {
        if seg == "worktrees" && i + 1 < components.len() {
            let next_seg = components[i + 1];
            // If followed by task-* or attempt-*, next_seg is the workspace identifier
            if i + 2 < components.len()
                && (components[i + 2].starts_with("task-")
                    || components[i + 2].starts_with("attempt-"))
            {
                // Each worktree owns an isolated workspace named after its origin repository.
                return sanitize_identifier(next_seg) + &worktree_suffix(client_root);
            }
        }
    }

    // 2. Check for local worktrees inside repository:
    // e.g. ".../<project_name>/.worktrees/..." or ".../<project_name>/worktrees/..."
    for (i, &seg) in components.iter().enumerate() {
        if (seg == ".worktrees" || seg == "worktrees") && i > 0 {
            let prev_seg = components[i - 1];
            // Disregard generic container prefixes
            if prev_seg != "Volumes"
                && prev_seg != "mnt"
                && prev_seg != "srv"
                && prev_seg != "home"
                && prev_seg != "var"
            {
                return sanitize_identifier(prev_seg) + &worktree_suffix(client_root);
            }
        }
    }

    // 3. Fallback: nearest ancestor that is not a task-* or attempt-* runner directory
    if let Some(pos) = components.iter().rposition(|&c| {
        !c.starts_with("task-") && !c.starts_with("attempt-") && c != "worktree" && c != "worktrees"
    }) {
        let name = components[pos];
        if !name.is_empty() {
            return sanitize_identifier(name);
        }
    }

    // 4. Fallback: folder name of client_root
    let fallback = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("default");

    sanitize_identifier(fallback)
}

pub fn sanitize_identifier(s: &str) -> String {
    let sanitized: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "workspace".to_string()
    } else {
        sanitized
    }
}

/// Resolve client workspace path or worktree path to the canonical server workspace root.
pub fn resolve_server_workspace(
    storage_root: &Path,
    client_root: &str,
    explicit_base_name: Option<&str>,
) -> PathBuf {
    let target_dir = server_workspace_path(storage_root, client_root, explicit_base_name);
    let _ = std::fs::create_dir_all(&target_dir);
    target_dir
}

/// The server workspace directory for a client, without creating it.
pub fn server_workspace_path(
    storage_root: &Path,
    client_root: &str,
    explicit_base_name: Option<&str>,
) -> PathBuf {
    let candidate_name = match explicit_base_name {
        Some(name) if !name.trim().is_empty() => sanitize_identifier(name.trim()),
        _ => extract_workspace_identifier(client_root),
    };
    storage_root.join(candidate_name)
}

/// Marker file whose mtime records the last handshake on a workspace directory.
pub const LAST_USED_MARKER: &str = ".prod-code-last-used";

/// Removes `<repo>--wt-*` workspace directories that have not been used for `max_age` and are
/// not loaded. A client whose worktree reappears simply resyncs. Returns the removed paths.
pub async fn prune_stale_worktree_dirs(
    storage_root: &Path,
    max_age: Duration,
    manager: &WorkspaceManager,
) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(storage_root) else {
        return removed;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !path.is_dir() || !name.contains("--wt-") {
            continue;
        }
        if manager.is_loaded(&path).await {
            continue;
        }
        let stamp = std::fs::metadata(path.join(LAST_USED_MARKER))
            .or_else(|_| std::fs::metadata(&path))
            .and_then(|m| m.modified())
            .ok();
        let idle = stamp
            .and_then(|t| now.duration_since(t).ok())
            .unwrap_or_default();
        if idle < max_age {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                tracing::info!(workspace = %path.display(), idle_days = idle.as_secs() / 86_400, "🧹 pruned stale worktree workspace");
                removed.push(path);
            }
            Err(e) => {
                tracing::warn!(error = %e, workspace = %path.display(), "failed to prune worktree workspace")
            }
        }
    }
    removed
}

/// Records a handshake on the workspace directory for [`prune_stale_worktree_dirs`].
pub fn touch_last_used(workspace_dir: &Path) {
    let marker = workspace_dir.join(LAST_USED_MARKER);
    let _ = std::fs::write(&marker, unix_now().to_string());
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

    #[tokio::test]
    async fn test_evict_idle_drops_only_idle_unused_workspaces() {
        let manager = WorkspaceManager::new();
        let idle = Arc::new(SharedWorkspace::new(
            PathBuf::from("/srv/ws/idle"),
            "rust".to_string(),
            None,
            None,
            None,
            None,
        ));
        idle.last_used.store(unix_now() - 7200, Ordering::Relaxed);
        let busy = Arc::new(SharedWorkspace::new(
            PathBuf::from("/srv/ws/busy"),
            "rust".to_string(),
            None,
            None,
            None,
            None,
        ));
        busy.last_used.store(unix_now() - 7200, Ordering::Relaxed);
        busy.active_sessions.store(1, Ordering::Relaxed);
        let fresh = Arc::new(SharedWorkspace::new(
            PathBuf::from("/srv/ws/fresh"),
            "rust".to_string(),
            None,
            None,
            None,
            None,
        ));
        manager.insert_ready_for_test(idle).await;
        manager.insert_ready_for_test(busy).await;
        manager.insert_ready_for_test(fresh).await;

        let evicted = manager.evict_idle(Duration::from_secs(3600)).await;
        assert_eq!(evicted, vec![PathBuf::from("/srv/ws/idle")]);
        assert_eq!(manager.loaded_count().await, 2);
        assert!(manager.is_loaded(Path::new("/srv/ws/busy")).await);
        assert!(!manager.is_loaded(Path::new("/srv/ws/idle")).await);
    }

    #[tokio::test]
    async fn test_prune_stale_worktree_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let storage = temp.path();
        let manager = WorkspaceManager::new();
        let old = storage.join("repo--wt-deadbeef");
        let recent = storage.join("repo--wt-cafebabe");
        let main = storage.join("repo");
        for d in [&old, &recent, &main] {
            std::fs::create_dir_all(d).unwrap();
            touch_last_used(d);
        }
        let long_ago = SystemTime::now() - Duration::from_secs(30 * 86_400);
        std::fs::File::options()
            .write(true)
            .open(old.join(LAST_USED_MARKER))
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        // An old main-repository directory is never pruned, only worktree copies.
        std::fs::File::options()
            .write(true)
            .open(main.join(LAST_USED_MARKER))
            .unwrap()
            .set_modified(long_ago)
            .unwrap();

        let removed =
            prune_stale_worktree_dirs(storage, Duration::from_secs(7 * 86_400), &manager).await;
        assert_eq!(removed, vec![old.clone()]);
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(main.exists());
    }

    #[test]
    fn test_resolve_server_workspace_generic_worktree_mapping() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = temp_dir.path();

        // 1. Nested runner worktree container: .../worktrees/<workspace_name>/task-123/attempt-0
        let wt1 = "/Volumes/worktrees/worktrees/repo-alpha/task-1531/attempt-0";
        let res1 = resolve_server_workspace(storage, wt1, None);
        assert_eq!(
            res1,
            storage.join(format!("repo-alpha{}", worktree_suffix(wt1)))
        );
        assert!(res1.is_dir(), "Workspace directory must be auto-created");
        let wt1b = "/Volumes/worktrees/worktrees/repo-alpha/task-1532/attempt-0";
        assert_ne!(resolve_server_workspace(storage, wt1b, None), res1);

        // 2. In-repo dot-worktrees pattern: .../project-beta/.worktrees/branch-1
        let wt2 = "/home/dev/projects/project-beta/.worktrees/branch-1";
        let res2 = resolve_server_workspace(storage, wt2, None);
        assert_eq!(
            res2,
            storage.join(format!("project-beta{}", worktree_suffix(wt2)))
        );
        assert!(res2.is_dir());

        // 3. Worktree with explicit base name provided by client
        let wt3 = "/Users/dev/scratch/temp-worktree";
        let res3 = resolve_server_workspace(storage, wt3, Some("core-service"));
        assert_eq!(res3, storage.join("core-service"));
        assert!(res3.is_dir());

        // 4. Standard repository folder
        let std_repo = "/Users/dev/workspace/payment-gateway";
        let res4 = resolve_server_workspace(storage, std_repo, None);
        assert_eq!(res4, storage.join("payment-gateway"));
        assert!(res4.is_dir());
    }
}
