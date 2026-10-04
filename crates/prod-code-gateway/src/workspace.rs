//! Workspace management: multi-tenant shared workspaces, leader-follower coalescing, and worktree views.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock, broadcast};

/// Unique identifier for a shared workspace based on its canonical root.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceKey(pub PathBuf);

/// Every in-memory Rust engine that answers for one workspace root: the main one, and the
/// validation engine once a validation session has loaded it. A file that changes on disk must
/// reach all of them, and the sync paths only ever see one [`SharedWorkspace`], so the list is
/// shared between the workspace and the validation view derived from it.
pub type RustEngines = Arc<std::sync::Mutex<Vec<Arc<Mutex<prod_code_engine_rust::RustEngine>>>>>;

/// Loads the in-process Rust engine of a root, on a blocking thread; tests put a slow one in
/// its place.
pub type RustLoader = Arc<dyn Fn(&Path) -> Result<prod_code_engine_rust::RustEngine> + Send + Sync>;

/// A loaded base workspace shared across multiple sessions/worktrees.
pub struct SharedWorkspace {
    pub key: WorkspaceKey,
    pub root: PathBuf,
    pub engine: String,
    pub active_sessions: AtomicUsize,
    /// Unix seconds of the last session registration or retirement, for idle eviction.
    pub last_used: AtomicU64,
    /// When the engine was loaded: a session is told its age, so that an empty answer from an
    /// engine still indexing is asked again and one from a warm engine is not (#381).
    pub loaded_at: std::time::Instant,
    pub direct_edit_eligible: AtomicBool,
    pub rust_engine: Option<Arc<Mutex<prod_code_engine_rust::RustEngine>>>,
    pub go_engine: Option<Arc<prod_code_engine_go::GoEngine>>,
    pub generic_engine: Option<Arc<prod_code_engine_generic::GenericLspEngine>>,
    pub backend: Option<Arc<crate::backend::BackendWorker>>,
    /// All the Rust engines for this root, `rust_engine` first; see [`RustEngines`].
    pub rust_engines: RustEngines,
    /// The second engine validation sessions run on, loaded by the first of them (#73).
    /// `None` inside once a load failed: validation then falls back to the main engine.
    validation: tokio::sync::OnceCell<Option<Arc<Mutex<prod_code_engine_rust::RustEngine>>>>,
    /// The second generic server validation sessions run on, started by the first of them.
    /// A full retained-document generation is dropped and replaced as one unit.
    generic_validation: Mutex<Option<Arc<prod_code_engine_generic::GenericLspEngine>>>,
    generic_validation_loaded: AtomicBool,
    /// A generic server has one process-wide document state. Validation sessions use it one
    /// at a time so parallel proposals cannot replace each other's overlays.
    pub generic_validation_session: Arc<tokio::sync::Mutex<()>>,
    /// Shared with the manager's primary loads so private validation servers use the same
    /// node-wide concurrency budget.
    engine_load_semaphore: Arc<tokio::sync::Semaphore>,
    /// If this workspace is an attached worktree overlay of a base workspace, keeps the base
    /// alive so its engine is not evicted while this worktree is active.
    pub base_workspace: Option<Arc<SharedWorkspace>>,
    /// Number of loaded worktrees attached to this base workspace.
    pub attached_worktrees: AtomicUsize,
    /// Whether this worktree overlay has already been detached from the base engine.
    pub detached: AtomicBool,
    /// Whether this workspace has been unloaded from the manager's active map.
    pub unloaded: AtomicBool,
    /// Whether this worktree overlay has been attached to the base validation engine.
    pub validation_attached: AtomicBool,
    /// Broadcast channel for active session dynamic rebalancing redirects.
    pub rebalance_tx: broadcast::Sender<(String, Option<String>)>,
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
        Self::with_base(root, engine, rust_engine, go_engine, generic_engine, backend, None)
    }

    pub fn with_base(
        root: PathBuf,
        engine: String,
        rust_engine: Option<Arc<Mutex<prod_code_engine_rust::RustEngine>>>,
        go_engine: Option<Arc<prod_code_engine_go::GoEngine>>,
        generic_engine: Option<Arc<prod_code_engine_generic::GenericLspEngine>>,
        backend: Option<Arc<crate::backend::BackendWorker>>,
        base_workspace: Option<Arc<SharedWorkspace>>,
    ) -> Self {
        let rust_engines = base_workspace
            .as_ref()
            .map(|b| Arc::clone(&b.rust_engines))
            .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(rust_engine.iter().cloned().collect())));
        let (rebalance_tx, _) = broadcast::channel(16);
        let rebalance_tx = base_workspace
            .as_ref()
            .map(|b| b.rebalance_tx.clone())
            .unwrap_or(rebalance_tx);
        let engine_load_semaphore = base_workspace
            .as_ref()
            .map(|base| Arc::clone(&base.engine_load_semaphore))
            .unwrap_or_else(|| {
                Arc::new(tokio::sync::Semaphore::new(default_max_concurrent_engine_loads()))
            });
        Self {
            key: WorkspaceKey(root.clone()),
            root,
            engine,
            active_sessions: AtomicUsize::new(0),
            last_used: AtomicU64::new(unix_now()),
            loaded_at: std::time::Instant::now(),
            direct_edit_eligible: AtomicBool::new(true),
            rust_engine,
            go_engine,
            generic_engine,
            backend,
            rust_engines,
            validation: tokio::sync::OnceCell::new(),
            generic_validation: Mutex::new(None),
            generic_validation_loaded: AtomicBool::new(false),
            generic_validation_session: Arc::default(),
            engine_load_semaphore,
            base_workspace,
            attached_worktrees: AtomicUsize::new(0),
            detached: AtomicBool::new(false),
            unloaded: AtomicBool::new(false),
            validation_attached: AtomicBool::new(false),
            rebalance_tx,
        }
    }

    /// Broadcasts a rebalance redirect request to all active sessions of this workspace.
    pub fn trigger_rebalance(&self, target_addr: String, reason: Option<String>) -> usize {
        self.rebalance_tx.send((target_addr, reason)).unwrap_or(0)
    }

    /// Subscribes to rebalance redirect requests for this workspace.
    pub fn subscribe_rebalance(&self) -> broadcast::Receiver<(String, Option<String>)> {
        self.rebalance_tx.subscribe()
    }

    /// Every Rust engine a change to a file under this root must reach.
    pub fn mirrored_rust_engines(&self) -> Vec<Arc<Mutex<prod_code_engine_rust::RustEngine>>> {
        self.rust_engines
            .lock()
            .map(|engines| engines.clone())
            .unwrap_or_default()
    }

    /// The view a validation session runs in: this workspace, answered by a second Rust
    /// engine that nothing but validation touches.
    ///
    /// A validation session opens proposed texts, pulls diagnostics and closes them. When the
    /// texts change what a widely imported file declares, both the overlay and its revert make
    /// rust-analyzer re-resolve the crates that import it, and that bill lands on the next
    /// query, whoever asks it — twenty seconds for a `references` after a dry run on this
    /// repository. On a second engine the bill stays there: the main engine never sees the
    /// overlay. The second engine is loaded by the first validation session, costs the memory
    /// of one more database, and is dropped with this workspace. If it cannot be loaded,
    /// validation runs on the main engine as before.
    ///
    /// The second engine is a new engine like any other: while `admission` has no memory for
    /// it, validation runs on the main engine, and a later validation session asks again.
    pub async fn validation_view(
        self: &Arc<Self>,
        admission: &Arc<crate::admission::Admission>,
    ) -> Result<Arc<SharedWorkspace>> {
        if self.generic_engine.is_some() {
            return self.generic_validation_view(admission).await;
        }
        if self.rust_engine.is_none() {
            return Ok(Arc::clone(self));
        }
        if let Some(base) = &self.base_workspace {
            let base_val = base.base_validation_view(admission).await?;
            let is_main = Arc::ptr_eq(&base_val, base)
                || base_val
                    .rust_engine
                    .as_ref()
                    .and_then(|val| self.rust_engine.as_ref().map(|main| Arc::ptr_eq(main, val)))
                    .unwrap_or(false);
            if let Some(val_engine) = &base_val.rust_engine
                && !is_main
                && !self.validation_attached.swap(true, Ordering::SeqCst)
            {
                let mut eng = val_engine.lock().await;
                if let Err(e) = eng.attach_worktree(&self.root) {
                    self.validation_attached.store(false, Ordering::SeqCst);
                    tracing::warn!(error = %e, workspace = ?self.root, "failed to attach worktree to validation engine");
                }
            }
            return Ok(Arc::new(SharedWorkspace {
                loaded_at: self.loaded_at,
                key: self.key.clone(),
                root: self.root.clone(),
                engine: self.engine.clone(),
                active_sessions: AtomicUsize::new(0),
                last_used: AtomicU64::new(unix_now()),
                direct_edit_eligible: AtomicBool::new(false),
                rust_engine: base_val.rust_engine.clone(),
                go_engine: None,
                generic_engine: None,
                backend: None,
                rust_engines: Arc::clone(&base_val.rust_engines),
                validation: tokio::sync::OnceCell::new(),
                generic_validation: Mutex::new(None),
                generic_validation_loaded: AtomicBool::new(false),
                generic_validation_session: Arc::clone(&self.generic_validation_session),
                engine_load_semaphore: Arc::clone(&self.engine_load_semaphore),
                base_workspace: Some(Arc::clone(base)),
                attached_worktrees: AtomicUsize::new(0),
                detached: AtomicBool::new(false),
                unloaded: AtomicBool::new(false),
                validation_attached: AtomicBool::new(!is_main),
                rebalance_tx: self.rebalance_tx.clone(),
            }));
        }
        self.base_validation_view(admission).await
    }

    async fn base_validation_view(
        self: &Arc<Self>,
        admission: &Arc<crate::admission::Admission>,
    ) -> Result<Arc<SharedWorkspace>> {
        let load: RustLoader = Arc::new(prod_code_engine_rust::RustEngine::load);
        let Some(engine) = self.validation_engine(admission, load).await else {
            return Ok(Arc::clone(self));
        };
        Ok(Arc::new(SharedWorkspace {
            loaded_at: self.loaded_at,
            key: self.key.clone(),
            root: self.root.clone(),
            engine: self.engine.clone(),
            active_sessions: AtomicUsize::new(0),
            last_used: AtomicU64::new(unix_now()),
            direct_edit_eligible: AtomicBool::new(false),
            rust_engine: Some(engine),
            go_engine: None,
            generic_engine: None,
            backend: None,
            rust_engines: Arc::clone(&self.rust_engines),
            validation: tokio::sync::OnceCell::new(),
            generic_validation: Mutex::new(None),
            generic_validation_loaded: AtomicBool::new(false),
            generic_validation_session: Arc::clone(&self.generic_validation_session),
            engine_load_semaphore: Arc::clone(&self.engine_load_semaphore),
            base_workspace: None,
            attached_worktrees: AtomicUsize::new(0),
            detached: AtomicBool::new(false),
            unloaded: AtomicBool::new(false),
            validation_attached: AtomicBool::new(false),
            rebalance_tx: self.rebalance_tx.clone(),
        }))
    }
}

impl SharedWorkspace {
    /// The validation engine, loaded with `load` by the first session that asks while the host
    /// has memory for it. The load runs in a task of its own: a session that stops waiting
    /// leaves it running with its reservation, and the next session waits for its engine
    /// instead of loading another beside it.
    async fn validation_engine(
        self: &Arc<Self>,
        admission: &Arc<crate::admission::Admission>,
        load: RustLoader,
    ) -> Option<Arc<Mutex<prod_code_engine_rust::RustEngine>>> {
        let this = Arc::clone(self);
        let admission = Arc::clone(admission);
        let load_semaphore = Arc::clone(&this.engine_load_semaphore);
        let loading = tokio::spawn(async move {
            let engines = Arc::clone(&this.rust_engines);
            let root = this.root.clone();
            this.validation
                .get_or_try_init(|| async move {
                    let reservation = admission.try_reserve("rust").map_err(|shortfall| {
                        let refused = crate::admission::CapacityRefused {
                            shortfall,
                            reclaimed: 0,
                        };
                        tracing::warn!(workspace = ?root, %refused, "validation engine not loaded; validating on the main engine");
                    })?;
                    let _load_permit = load_semaphore.acquire_owned().await.map_err(|_| ())?;
                    let load_root = root.clone();
                    let loaded = tokio::task::spawn_blocking(move || load(&load_root)).await;
                    reservation.release_after_settling();
                    Ok::<_, ()>(match loaded {
                        Ok(Ok(mut engine)) => {
                            engine.set_label("validation");
                            let engine = Arc::new(Mutex::new(engine));
                            if let Ok(mut all) = engines.lock() {
                                all.push(Arc::clone(&engine));
                            }
                            tracing::info!(workspace = ?root, "validation engine loaded");
                            // Nothing is warm after a load, and the files modified last are the
                            // ones an agent validates next (#233).
                            crate::priming::warm_in_background(
                                Arc::clone(&engine),
                                root.clone(),
                                crate::priming::recent_rust_files(
                                    &root,
                                    crate::priming::RECENT_FILES,
                                ),
                            );
                            Some(engine)
                        }
                        Ok(Err(err)) => {
                            tracing::warn!(error = %err, workspace = ?root, "validation engine failed to load; validating on the main engine");
                            None
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, workspace = ?root, "validation engine load panicked; validating on the main engine");
                            None
                        }
                    })
                })
                .await
                .ok()
                .cloned()
                .flatten()
        });
        loading.await.ok().flatten()
    }

    /// The validation view of a generic-language workspace: this workspace, answered by a
    /// second language server that nothing but validation touches (#293, #466).
    ///
    /// clangd keeps a closed document in its index as it was last built, and builds a source
    /// against the preamble it already has before it notices that a header changed back. So a
    /// validation session's proposed texts went on answering `references` and diagnostics in
    /// the server every other session asks, after the session had closed them. On a second
    /// server they stay there. It indexes nothing in the background, costs one more clangd
    /// while the workspace is loaded, and is dropped with it. If it cannot start, validation
    /// fails clearly rather than putting proposal overlays into the main server.
    async fn generic_validation_view(
        self: &Arc<Self>,
        admission: &Arc<crate::admission::Admission>,
    ) -> Result<Arc<SharedWorkspace>> {
        // Started in a task of its own, like the Rust validation engine: a session that stops
        // waiting leaves the start, and its reservation, to finish for the next one.
        let this = Arc::clone(self);
        let admission = Arc::clone(admission);
        let load_semaphore = Arc::clone(&this.engine_load_semaphore);
        let starting = tokio::spawn(async move {
            let root = this.root.clone();
            let engine = this.engine.clone();
            let config = if engine == "cpp" {
                prod_code_engine_generic::GenericLspConfig::for_cpp_validation()
            } else {
                this.generic_engine
                    .as_ref()
                    .expect("generic engine")
                    .config
                    .clone()
            };
            let mut slot = this.generic_validation.lock().await;
            if let Some(current) = slot.as_ref()
                && current.is_alive()
                && current.accepts_documents()
            {
                return Ok(Arc::clone(current));
            }
            if slot.take().is_some() {
                this.generic_validation_loaded
                    .store(false, Ordering::Relaxed);
            }
            let reservation = admission.try_reserve(&engine).map_err(|shortfall| {
                crate::admission::CapacityRefused {
                    shortfall,
                    reclaimed: 0,
                }
            })?;
            let _load_permit = load_semaphore
                .acquire_owned()
                .await
                .context("engine load semaphore closed")?;
            let started = prod_code_engine_generic::GenericLspEngine::spawn(&root, config).await;
            reservation.release_after_settling();
            let started =
                Arc::new(started.with_context(|| {
                    format!("private {engine} validation server failed to start")
                })?);
            if engine == "swift" {
                wait_for_swift_build_settings(&started, &root).await;
            }
            tracing::info!(workspace = ?root, "generic validation server started");
            *slot = Some(Arc::clone(&started));
            this.generic_validation_loaded
                .store(true, Ordering::Relaxed);
            Ok::<_, anyhow::Error>(started)
        });
        let engine = starting
            .await
            .context("private generic validation server start task ended")??;
        Ok(Arc::new(SharedWorkspace {
            loaded_at: self.loaded_at,
            key: self.key.clone(),
            root: self.root.clone(),
            engine: self.engine.clone(),
            active_sessions: AtomicUsize::new(0),
            last_used: AtomicU64::new(unix_now()),
            direct_edit_eligible: AtomicBool::new(false),
            rust_engine: None,
            go_engine: None,
            generic_engine: Some(engine),
            backend: None,
            rust_engines: Arc::clone(&self.rust_engines),
            validation: tokio::sync::OnceCell::new(),
            generic_validation: Mutex::new(None),
            generic_validation_loaded: AtomicBool::new(false),
            generic_validation_session: Arc::clone(&self.generic_validation_session),
            engine_load_semaphore: Arc::clone(&self.engine_load_semaphore),
            base_workspace: None,
            attached_worktrees: AtomicUsize::new(0),
            detached: AtomicBool::new(false),
            unloaded: AtomicBool::new(false),
            validation_attached: AtomicBool::new(false),
            rebalance_tx: self.rebalance_tx.clone(),
        }))
    }

    pub fn touch(&self) {
        self.last_used.store(unix_now(), Ordering::Relaxed);
    }

    /// What unloading this workspace is counted to free: its engine, and its validation engine
    /// once one runs.
    fn reclaimable(&self, admission: &crate::admission::Admission) -> u64 {
        if self.base_workspace.is_some() || self.attached_worktrees.load(Ordering::Relaxed) > 0 {
            return 0;
        }
        let engines = if self.validation.get().is_some_and(Option::is_some)
            || self.generic_validation_loaded.load(Ordering::Relaxed)
        {
            2
        } else {
            1
        };
        admission.reserve_for(&self.engine).saturating_mul(engines)
    }

    /// Whether a language server this workspace answers from has exited. One that crashed (the
    /// TypeScript server on a file of another language) would otherwise answer every later
    /// query with its exit, until the gateway restarted (#355).
    pub fn has_dead_server(&self) -> bool {
        self.generic_engine
            .as_ref()
            .is_some_and(|e| !e.is_alive() || !e.accepts_documents())
            || self.go_engine.as_ref().is_some_and(|e| !e.is_alive())
            || self.backend.as_ref().is_some_and(|e| !e.is_alive())
    }

    /// Whether the next session asking for `engine` may be handed this workspace: it was loaded
    /// for that engine and its language server is still running.
    fn reusable_for(&self, engine: &str) -> bool {
        self.engine == engine && !self.has_dead_server()
    }

    /// Tells this workspace's language servers (gopls, or the generic server and the C/C++
    /// validation clangd once it runs) which files a sync created, rewrote or removed on disk,
    /// as `workspace/didChangeWatchedFiles` (#317). Paths outside the workspace root are left
    /// out.
    pub async fn notify_watched_files(&self, changes: &[(PathBuf, WatchedChange)]) {
        let events = watched_events(&self.root, changes);
        if events.is_empty() {
            return;
        }
        let params = serde_json::json!({ "changes": events });
        const METHOD: &str = "workspace/didChangeWatchedFiles";
        if let Some(go) = &self.go_engine
            && let Err(err) = go.send_notification(METHOD, params.clone()).await
        {
            tracing::warn!(error = %err, workspace = ?self.root, "gopls was not told about synced files");
        }
        let validation = self.generic_validation.lock().await.clone();
        for server in self.generic_engine.iter().chain(validation.iter()) {
            if let Err(err) = server.send_notification(METHOD, params.clone()).await {
                tracing::warn!(error = %err, workspace = ?self.root, "language server was not told about synced files");
            }
        }
    }

    /// Detaches this worktree overlay from its base engine and decrements the base workspace's
    /// attached worktree count. Executes at most once across the lifetime of this workspace.
    pub async fn detach_overlay(&self) {
        if self.base_workspace.is_none() {
            return;
        }
        if self.detached.swap(true, Ordering::SeqCst) {
            return;
        }
        let val_attached = self.validation_attached.load(Ordering::SeqCst);
        if let Some(ref base) = self.base_workspace {
            base.attached_worktrees.fetch_sub(1, Ordering::Relaxed);
            for eng_arc in self.mirrored_rust_engines() {
                let mut eng = eng_arc.lock().await;
                eng.detach_worktree(&self.root);
            }
            if val_attached
                && let Some(Some(val_eng)) = base.validation.get()
            {
                let mut eng = val_eng.lock().await;
                eng.detach_worktree(&self.root);
            }
        }
    }
}

/// The `FileEvent`s of `workspace/didChangeWatchedFiles` for the `changes` under `root`.
pub fn watched_events(root: &Path, changes: &[(PathBuf, WatchedChange)]) -> Vec<serde_json::Value> {
    changes
        .iter()
        .filter(|(path, _)| path.starts_with(root))
        .map(|(path, kind)| {
            serde_json::json!({ "uri": prod_code_protocol::path::file_uri(path), "type": *kind as u8 })
        })
        .collect()
}

/// How a sync changed a file on disk, numbered as LSP's `FileChangeType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchedChange {
    Created = 1,
    Changed = 2,
    Deleted = 3,
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
    /// The workspace the session's queries run against: the loaded one, or the validation view
    /// derived from it.
    pub workspace: Arc<SharedWorkspace>,
    /// The loaded workspace the session is counted against, for idle eviction.
    pub accounted: Arc<SharedWorkspace>,
    pub is_single_owner: Arc<AtomicBool>,
    pub direct_edit_open_files: Arc<std::sync::Mutex<std::collections::HashMap<PathBuf, String>>>,
    lease: Option<WorkspaceLease>,
    owner: Option<WorktreeOwner>,
}

impl SessionView {
    pub fn is_single_owner(&self) -> bool {
        self.is_single_owner.load(Ordering::SeqCst)
    }
}

/// One counted attachment to a loaded workspace. Until it is transferred into a
/// [`SessionView`], dropping the handshake future returns the count automatically.
pub struct WorkspaceLease {
    workspace: Option<Arc<SharedWorkspace>>,
    _base_lease: Option<Box<WorkspaceLease>>,
}

impl WorkspaceLease {
    fn acquire(workspace: Arc<SharedWorkspace>) -> Self {
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

type WorktreeOwners = Arc<std::sync::Mutex<HashMap<PathBuf, WorktreeEntry>>>;

struct WorktreeOwner {
    root: PathBuf,
    session_id: u64,
    owners: WorktreeOwners,
}

impl Drop for WorktreeOwner {
    fn drop(&mut self) {
        let mut owners = self.owners.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = owners.get_mut(&self.root) {
            entry.count = entry.count.saturating_sub(1);
            if let Some(lease) = &entry.direct_edit_lease
                && lease.session_id == self.session_id {
                    entry.direct_edit_lease = None;
                }
            if entry.count == 0 {
                owners.remove(&self.root);
            }
        }
    }
}

impl SessionView {
    fn start_retirement(&mut self) -> Option<tokio::task::JoinHandle<()>> {
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

    async fn retire(mut self) {
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

async fn clear_session_overlays(
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

/// State of an in-flight workspace load.
enum LoadState {
    Loading(broadcast::Sender<Result<Arc<SharedWorkspace>, String>>),
    Ready(Arc<SharedWorkspace>),
}

/// Default bound on concurrent cold engine loads (#408).
/// On high-core nodes (e.g. 128 cores), loading 16 engines simultaneously starves CPU and I/O caches
/// and drives first-query response times past timeouts. Limiting in-flight loads ensures the first
/// workspaces load quickly and answer within their budget.
pub fn default_max_concurrent_engine_loads() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    if cpus >= 32 {
        8
    } else if cpus >= 8 {
        4
    } else {
        2
    }
}

/// Thread-safe manager coordinating workspace lifecycle and leader-follower loading.
pub struct WorkspaceManager {
    workspaces: RwLock<HashMap<WorkspaceKey, LoadState>>,
    worktree_owners: WorktreeOwners,
    /// The language servers of editors' sessions, which run outside the shared workspaces.
    pub editor_servers: crate::editor_proxy::EditorServers,
    /// Whether the host has memory for another engine (#433).
    admission: Arc<crate::admission::Admission>,
    rust_loader: RustLoader,
    load_semaphore: Arc<tokio::sync::Semaphore>,
}

/// How long an engine must have been without a session before a load that finds no memory may
/// unload it. An agent's commands come in bursts with short gaps between them, and an engine
/// unloaded in such a gap is loaded again, cold, by the next command.
pub const RECLAIM_MIN_IDLE: Duration = Duration::from_secs(120);

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceManager {
    /// A manager that admits every new engine. The server admits against the host's memory
    /// ([`Self::with_admission`]); a test's outcome must not depend on the node it runs on.
    pub fn new() -> Self {
        Self::with_admission(Arc::new(crate::admission::Admission::unbounded()))
    }

    pub fn with_admission(admission: Arc<crate::admission::Admission>) -> Self {
        Self::with_admission_and_concurrency(admission, default_max_concurrent_engine_loads())
    }

    pub fn with_admission_and_concurrency(
        admission: Arc<crate::admission::Admission>,
        max_concurrent_loads: usize,
    ) -> Self {
        let max_concurrent_loads = if max_concurrent_loads == 0 {
            default_max_concurrent_engine_loads()
        } else {
            max_concurrent_loads
        };
        Self {
            workspaces: RwLock::new(HashMap::new()),
            worktree_owners: Arc::new(std::sync::Mutex::new(HashMap::new())),
            editor_servers: crate::editor_proxy::EditorServers::default(),
            admission,
            rust_loader: Arc::new(prod_code_engine_rust::RustEngine::load),
            load_semaphore: Arc::new(tokio::sync::Semaphore::new(max_concurrent_loads)),
        }
    }

    #[cfg(test)]
    pub fn with_max_concurrent_loads(mut self, max: usize) -> Self {
        self.load_semaphore = Arc::new(tokio::sync::Semaphore::new(max));
        self
    }

    #[cfg(test)]
    fn with_rust_loader(mut self, rust_loader: RustLoader) -> Self {
        self.rust_loader = rust_loader;
        self
    }

    pub fn admission(&self) -> &Arc<crate::admission::Admission> {
        &self.admission
    }

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
    async fn admit(
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

    /// Retrieve or load a shared workspace using leader-follower coalescing.
    ///
    /// If another session is already loading this workspace, current session becomes
    /// a follower and awaits the leader's result without duplicating compiler work.
    pub async fn get_or_load(
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

    async fn get_or_load_direct(
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
            tokio::spawn(Arc::clone(self).lead(
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
    async fn get_or_load_worktree(
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
                tokio::spawn(Arc::clone(self).lead(
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

    /// Leads the load of `key` to its end: makes it Ready, or takes its Loading entry out of
    /// the map when it failed or was refused, then answers the leader, if it still waits, and
    /// the followers. Everything that could panic, dropping the replaced engines among it, runs
    /// in a task of its own, so that a load that panics is answered too and never leaves its
    /// Loading entry behind.
    async fn lead(
        self: Arc<Self>,
        key: WorkspaceKey,
        engine: String,
        tx: broadcast::Sender<Result<Arc<SharedWorkspace>, String>>,
        replaced: Vec<Arc<SharedWorkspace>>,
        leader: tokio::sync::oneshot::Sender<Result<WorkspaceLease>>,
    ) {
        let manager = Arc::clone(&self);
        let root = key.0.clone();
        let loading = tokio::spawn(async move {
            release(replaced).await;
            manager.load_as_leader(root, engine).await
        });
        let loaded = loading
            .await
            .unwrap_or_else(|err| Err(anyhow::anyhow!("the workspace load panicked: {err}")));
        match loaded {
            Ok((ws, reservation)) => {
                reservation.release_after_settling();
                let mut guard = self.workspaces.write().await;
                if !matches!(guard.get(&key), Some(LoadState::Loading(t)) if t.same_channel(&tx)) {
                    drop(guard);
                    let reason = "the workspace was unloaded while its engine was loading; retry the request";
                    let _ = tx.send(Err(reason.to_string()));
                    let _ = leader.send(Err(anyhow::anyhow!(reason)));
                    return;
                }
                let session =
                    (!leader.is_closed()).then(|| WorkspaceLease::acquire(Arc::clone(&ws)));
                guard.insert(key, LoadState::Ready(Arc::clone(&ws)));
                drop(guard);
                if let Some(session) = session {
                    let _ = leader.send(Ok(session));
                }
                let _ = tx.send(Ok(ws));
            }
            Err(err) => {
                {
                    let mut guard = self.workspaces.write().await;
                    if matches!(guard.get(&key), Some(LoadState::Loading(t)) if t.same_channel(&tx))
                    {
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
    async fn load_as_leader(
        self: Arc<Self>,
        workspace_root: PathBuf,
        engine: String,
    ) -> Result<(Arc<SharedWorkspace>, crate::admission::Reservation)> {
        let workspace_root = workspace_root.as_path();
        let engine = engine.as_str();
        // A new engine loads only while the host has memory for it (#433); sessions of engines
        // already loaded never get here.
        let reservation = self
            .admit(engine, &WorkspaceKey(workspace_root.to_path_buf()))
            .await
            .map_err(|refused| {
                tracing::warn!(workspace = ?workspace_root, engine, %refused, "🚫 [CAPACITY] refused to load a new engine");
                anyhow::Error::new(refused)
            })?;

        // Limit concurrent in-flight engine loads so parallel worktrees do not starve CPU/IO caches (#408).
        let _load_permit = self
            .load_semaphore
            .acquire()
            .await
            .map_err(|e| anyhow::anyhow!("engine load semaphore closed: {e}"))?;

        tracing::info!(workspace = ?workspace_root, engine, "Leader starting workspace load");
        let mut rust_engine = None;
        let mut go_engine = None;
        let mut generic_engine = None;
        let mut backend = None;

        match engine {
            "rust" => {
                let ws_path = workspace_root.to_path_buf();
                let load = Arc::clone(&self.rust_loader);
                let loaded_engine = tokio::task::spawn_blocking(move || {
                    // Every worktree copy builds into its own target directory: worktrees
                    // never share cargo state or wait on each other's build lock.
                    load(&ws_path)
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
            "cpp" => {
                warm_cmake_compile_commands(workspace_root).await;
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_cpp(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (C/C++ clangd) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn clangd; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "swift" => {
                let mut config = prod_code_engine_generic::GenericLspConfig::for_swift();
                for (k, v) in crate::swift_cache::swift_module_cache_env() {
                    config.env.insert(k, v);
                }
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    config,
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Swift sourcekit-lsp) active");
                        wait_for_swift_build_settings(&generic_eng, workspace_root).await;
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn sourcekit-lsp; falling back to subprocess");
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
            "java" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_java(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Java) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Java LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "kotlin" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_kotlin(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Kotlin) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Kotlin LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "csharp" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_csharp(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (C#) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn C# LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "php" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_php(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (PHP) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn PHP LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "ruby" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_ruby(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Ruby) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Ruby LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "dart" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_dart(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Dart) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Dart LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "zig" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_zig(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Zig) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Zig LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "elixir" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_elixir(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Elixir) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Elixir LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "scala" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_scala(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Scala) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Scala LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "lua" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_lua(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Lua) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Lua LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "haskell" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_haskell(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Haskell) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Haskell LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "ocaml" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_ocaml(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (OCaml) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn OCaml LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "clojure" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_clojure(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Clojure) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Clojure LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "julia" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_julia(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Julia) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Julia LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "shell" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_shell(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Shell) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Shell LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "r" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_r(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (R) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn R LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "erlang" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_erlang(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Erlang) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Erlang LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "fsharp" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_fsharp(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (F#) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn F# LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "perl" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_perl(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Perl) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Perl LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "solidity" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_solidity(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Solidity) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Solidity LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "nim" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_nim(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Nim) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Nim LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "d" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_d(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (D) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn D LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "fortran" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_fortran(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Fortran) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Fortran LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "sql" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_sql(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (SQL) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn SQL LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "graphql" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_graphql(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (GraphQL) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn GraphQL LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "protobuf" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_protobuf(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Protobuf) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Protobuf LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "crystal" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_crystal(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Crystal) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Crystal LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "groovy" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_groovy(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Groovy) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Groovy LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "ada" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_ada(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Ada) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Ada LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "v" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_v(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (V) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn V LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "racket" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_racket(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Racket) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Racket LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "terraform" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_terraform(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Terraform) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Terraform LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "nix" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_nix(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Nix) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Nix LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "markdown" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_markdown(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Markdown) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Markdown LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "yaml" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_yaml(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (YAML) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn YAML LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "toml" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_toml(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (TOML) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn TOML LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "json" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_json(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (JSON) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn JSON LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "html" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_html(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (HTML) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn HTML LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "css" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_css(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (CSS) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn CSS LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "dockerfile" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_dockerfile(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Dockerfile) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Dockerfile LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "svelte" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_svelte(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Svelte) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Svelte LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "vue" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_vue(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Vue) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Vue LSP; falling back to subprocess");
                        backend = crate::backend::BackendWorker::spawn(workspace_root, engine)
                            .await
                            .ok()
                            .map(Arc::new);
                    }
                }
            }
            "assembly" => {
                match prod_code_engine_generic::GenericLspEngine::spawn(
                    workspace_root,
                    prod_code_engine_generic::GenericLspConfig::for_assembly(),
                )
                .await
                {
                    Ok(generic_eng) => {
                        tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine (Assembly) active");
                        generic_engine = Some(Arc::new(generic_eng));
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn Assembly LSP; falling back to subprocess");
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

        let mut workspace = SharedWorkspace::new(
            workspace_root.to_path_buf(),
            engine.to_string(),
            rust_engine,
            go_engine,
            generic_engine,
            backend,
        );
        workspace.engine_load_semaphore = Arc::clone(&self.load_semaphore);
        let ws = Arc::new(workspace);
        Ok((ws, reservation))
    }

    /// Register a session's view over a worktree.
    ///
    /// Determines whether the session is the sole owner of this worktree path
    /// to activate the single-owner direct-edit fast path. If a second session
    /// connects to the same worktree, direct-edit exclusivity is revoked and any
    /// in-memory direct edits are migrated to session overlays before admitting the new session.
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
            Self::migrate_direct_edits_to_overlays(prev).await;
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

/// Drops unloaded workspaces on a blocking thread, after the map's lock is released: freeing an
/// analysis database takes a while, and must hold up neither other sessions nor a runtime
/// worker. Engines a session still holds live on until it ends.
async fn release(workspaces: Vec<Arc<SharedWorkspace>>) {
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

/// If `workspace_root` is a worktree copy (e.g. `<repo>--wt-<hash>` or git worktree with `.git` pointer), finds the base checkout root.
pub fn split_worktree_base(workspace_root: &Path) -> Option<PathBuf> {
    for ancestor in workspace_root.ancestors() {
        let Some(name) = ancestor.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        // 1. Server worktree naming: `<base>--wt-<hash>`
        if let Some((base_name, _)) = name.split_once("--wt-")
            && !base_name.is_empty()
        {
            let base_parent = ancestor.parent()?;
            let base_dir = base_parent.join(base_name);
            let relative = workspace_root.strip_prefix(ancestor).ok()?;
            let candidate = base_dir.join(relative);
            if base_dir.exists() {
                return Some(candidate);
            }
        }

        // 2. Standard Git worktree: ancestor contains a `.git` file with `gitdir:`
        let git_file = ancestor.join(".git");
        if git_file.is_file()
            && let Ok(content) = std::fs::read_to_string(&git_file)
        {
            for line in content.lines() {
                if let Some(gitdir_raw) = line.trim().strip_prefix("gitdir: ") {
                    let mut gitdir_path = PathBuf::from(gitdir_raw.trim());
                    if gitdir_path.is_relative() {
                        gitdir_path = ancestor.join(&gitdir_path);
                    }
                    if let Ok(canon) = gitdir_path.canonicalize() {
                        gitdir_path = canon;
                    }
                    for anc in gitdir_path.ancestors() {
                        if anc.file_name().and_then(|n| n.to_str()) == Some(".git")
                            && let Some(base_repo) = anc.parent()
                            && base_repo.exists()
                            && base_repo != ancestor
                            && let Ok(relative) = workspace_root.strip_prefix(ancestor)
                        {
                            let candidate = base_repo.join(relative);
                            return Some(candidate);
                        }
                    }
                }
            }
        }
    }

    None
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
    // Go tools skip a directory whose name begins with `.` or `_`: a copy named so hid its own
    // module from gopls, whose first `workspace/symbol` found nothing (#391).
    let sanitized = match sanitized.chars().next() {
        Some('.') => format!("dot-{}", sanitized.trim_start_matches('.')),
        Some('_') => format!("under-{}", sanitized.trim_start_matches('_')),
        _ => sanitized,
    };
    if sanitized.is_empty() {
        "workspace".to_string()
    } else {
        sanitized
    }
}

/// How long a workspace directory has gone unused: since its last-used marker, or the directory
/// itself when it has none.
fn idle_for(path: &Path, now: SystemTime) -> Duration {
    std::fs::metadata(path.join(LAST_USED_MARKER))
        .or_else(|_| std::fs::metadata(path))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| now.duration_since(t).ok())
        .unwrap_or_default()
}

/// The share of the filesystem holding `path` that is free for use (0.0 to 1.0); `None` when it
/// cannot be read.
pub fn free_share(path: &Path) -> Option<f64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statvfs` only writes the struct it is given, and the path is NUL-terminated.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let total = stat.f_blocks as f64 * stat.f_frsize as f64;
    (total > 0.0).then(|| stat.f_bavail as f64 * stat.f_frsize as f64 / total)
}

/// The free and total bytes of the filesystem holding `path`; `None` when it cannot be read (#809, #810).
pub fn free_and_total_bytes(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statvfs` only writes the struct it is given, and the path is NUL-terminated.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let total = stat.f_blocks as u64 * stat.f_frsize as u64;
    let free = stat.f_bavail as u64 * stat.f_frsize as u64;
    Some((free, total))
}

/// How long a worktree copy must have gone unused before it may be removed to free space: one in
/// use between two sessions stays.
const SPACE_PRUNE_MIN_IDLE: Duration = Duration::from_secs(3600);

/// Removes idle `<repo>--wt-*` copies that are not loaded, oldest first, while the storage
/// filesystem has less than `min_free` (a share of its size) free, however young they are: they
/// are rebuildable, and a client whose worktree comes back resyncs. Forty-seven of them filled a
/// 913 GB disk in two days, well before any was seven days idle, and the full disk then truncated
/// synced files (#385, #386). `free` reads the free share. Returns the removed paths.
pub async fn prune_worktree_dirs_for_space(
    storage_root: &Path,
    min_free: f64,
    manager: &WorkspaceManager,
    free: impl Fn(&Path) -> Option<f64>,
) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Some(mut share) = free(storage_root) else {
        return removed;
    };
    if share >= min_free {
        return removed;
    }
    let Ok(entries) = std::fs::read_dir(storage_root) else {
        return removed;
    };
    let now = SystemTime::now();
    let mut candidates: Vec<(Duration, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !path.is_dir() || !name.contains("--wt-") || manager.is_loaded(&path).await {
            continue;
        }
        let idle = idle_for(&path, now);
        if idle >= SPACE_PRUNE_MIN_IDLE {
            candidates.push((idle, path));
        }
    }
    candidates.sort_by_key(|(idle, _)| std::cmp::Reverse(*idle));
    for (idle, path) in candidates {
        if share >= min_free {
            break;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                let before = share;
                share = free(storage_root).unwrap_or(share);
                tracing::info!(
                    workspace = %path.display(),
                    idle_hours = idle.as_secs() / 3600,
                    free_before = %format!("{:.1}%", before * 100.0),
                    free_after = %format!("{:.1}%", share * 100.0),
                    "🧹 pruned a worktree workspace to free disk space"
                );
                removed.push(path);
            }
            Err(e) => {
                tracing::warn!(error = %e, workspace = %path.display(), "failed to prune worktree workspace")
            }
        }
    }
    if share < min_free {
        tracing::warn!(
            free = %format!("{:.1}%", share * 100.0),
            wanted = %format!("{:.1}%", min_free * 100.0),
            "storage is still low on space after pruning idle worktree copies"
        );
    }
    removed
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
        let idle = idle_for(&path, now);
        if idle < max_age {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                tracing::info!(
                    workspace = %path.display(),
                    idle_hours = idle.as_secs() / 3600,
                    idle_secs = idle.as_secs(),
                    "🧹 pruned stale worktree workspace"
                );
                removed.push(path);
            }
            Err(e) => {
                tracing::warn!(error = %e, workspace = %path.display(), "failed to prune worktree workspace")
            }
        }
    }
    removed
}

/// Removes main (non-worktree) workspace directories that have not been used for `max_age`,
/// are not loaded, and have no active or loaded worktree copies. Returns the removed paths.
pub async fn prune_stale_main_workspace_dirs(
    storage_root: &Path,
    max_age: Duration,
    worktree_max_age: Duration,
    manager: &WorkspaceManager,
) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(storage_root) else {
        return removed;
    };
    let now = SystemTime::now();
    let all_paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();

    for path in &all_paths {
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip hidden directories (e.g. .prod-code-shadow, .git)
        if name.starts_with('.') || name == "lost+found" {
            continue;
        }
        // Worktrees are handled separately by prune_stale_worktree_dirs
        if name.contains("--wt-") {
            continue;
        }
        // Main workspace must not be loaded in memory
        if manager.is_loaded(path).await {
            continue;
        }
        let idle = idle_for(path, now);
        if idle < max_age {
            continue;
        }

        // Base repository protection:
        // Check if any worktree of this base repo is currently loaded or still active on disk.
        let wt_prefix = format!("{name}--wt-");
        let mut has_active_worktree = false;
        for other in &all_paths {
            let Some(other_name) = other.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if other.is_dir() && other_name.starts_with(&wt_prefix) {
                // If the worktree is loaded in memory, the base repo must not be pruned
                if manager.is_loaded(other).await {
                    has_active_worktree = true;
                    break;
                }
                // If worktree pruning is disabled (worktree_max_age == 0) or worktree was used within worktree_max_age,
                // the worktree is still active on disk, so protect the base repo.
                if worktree_max_age.is_zero() || idle_for(other, now) < worktree_max_age {
                    has_active_worktree = true;
                    break;
                }
            }
        }

        if has_active_worktree {
            tracing::debug!(
                workspace = %path.display(),
                "main workspace is idle but protected by active worktrees"
            );
            continue;
        }

        match std::fs::remove_dir_all(path) {
            Ok(()) => {
                tracing::info!(
                    workspace = %path.display(),
                    idle_hours = idle.as_secs() / 3600,
                    idle_secs = idle.as_secs(),
                    "🧹 pruned stale main workspace"
                );
                removed.push(path.clone());
            }
            Err(e) => {
                tracing::warn!(error = %e, workspace = %path.display(), "failed to prune main workspace")
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

/// Persists a specific timestamp as the last-used time on the workspace directory.
pub fn touch_last_used_at(workspace_dir: &Path, ts: u64) {
    let marker = workspace_dir.join(LAST_USED_MARKER);
    let _ = std::fs::write(&marker, ts.to_string());
    if let Ok(file) = std::fs::File::options().write(true).open(&marker) {
        let system_time = SystemTime::UNIX_EPOCH + Duration::from_secs(ts);
        let _ = file.set_modified(system_time);
    }
}

/// File in a workspace directory that lists, one per line, the files the gateway removed from
/// its copy because a command changed them after its client left and their old contents were
/// not kept (#262). It lives on disk so that a gateway restart does not forget them: the client
/// still believes those files are on the node, and only this list makes it send them again.
pub const STALE_MARKER: &str = ".prod-code-stale";

/// The paths recorded by [`record_stale_paths`] for the workspace, sorted.
pub fn stale_paths(workspace_dir: &Path) -> Vec<String> {
    std::fs::read_to_string(workspace_dir.join(STALE_MARKER))
        .map(|text| {
            text.lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default()
}

fn write_stale_paths(workspace_dir: &Path, paths: &std::collections::BTreeSet<String>) {
    let marker = workspace_dir.join(STALE_MARKER);
    if paths.is_empty() {
        let _ = std::fs::remove_file(&marker);
        return;
    }
    let mut text = String::new();
    for path in paths {
        text.push_str(path);
        text.push('\n');
    }
    if let Err(e) = std::fs::write(&marker, text) {
        tracing::warn!(error = %e, marker = %marker.display(), "failed to record stale files");
    }
}

/// Adds `paths` to the workspace's stale files, which every handshake and sync answer reports
/// until the client has sent them again.
pub fn record_stale_paths(workspace_dir: &Path, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let mut all: std::collections::BTreeSet<String> =
        stale_paths(workspace_dir).into_iter().collect();
    all.extend(paths.iter().cloned());
    write_stale_paths(workspace_dir, &all);
}

/// Forgets every stale file of the workspace: after a manifest probe the copy holds exactly
/// the files the client listed, or asks for the ones it lacks, so none of them is lost anymore.
pub fn forget_stale_paths(workspace_dir: &Path) {
    let _ = std::fs::remove_file(workspace_dir.join(STALE_MARKER));
}

/// What each file of a workspace copy last received from a client sync, and when: the hash of
/// its text, or `None` for a deletion. A restore after a lost client (#262) must not put a
/// command's old text back over a newer one that a sync delivered while the command ran; the
/// client's watermark already counts that newer text as being on the node.
type SyncedFiles = HashMap<String, (std::time::Instant, Option<u64>)>;

static SYNCED: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, SyncedFiles>>> =
    std::sync::LazyLock::new(Default::default);

/// Notes that a sync just wrote (or deleted) `files` in the workspace copy.
pub fn record_synced(workspace_dir: &Path, files: &[(String, Option<u64>)]) {
    if files.is_empty() {
        return;
    }
    let now = std::time::Instant::now();
    let mut all = SYNCED.lock().unwrap_or_else(|e| e.into_inner());
    let known = all.entry(workspace_dir.to_path_buf()).or_default();
    for (path, hash) in files {
        known.insert(path.clone(), (now, *hash));
    }
}

/// The files a sync delivered to the workspace copy at or after `since`, with the hash of the
/// text it wrote (`None` for a deletion).
pub fn synced_since(
    workspace_dir: &Path,
    since: std::time::Instant,
) -> HashMap<String, Option<u64>> {
    let all = SYNCED.lock().unwrap_or_else(|e| e.into_inner());
    all.get(workspace_dir)
        .map(|known| {
            known
                .iter()
                .filter(|(_, (at, _))| *at >= since)
                .map(|(path, (_, hash))| (path.clone(), *hash))
                .collect()
        })
        .unwrap_or_default()
}

/// Removes the paths a sync just carried from the workspace's stale files, since the copy now
/// holds the checkout's version of them, and returns the ones still recorded.
pub fn clear_stale_paths<'a>(
    workspace_dir: &Path,
    arrived: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut all: std::collections::BTreeSet<String> =
        stale_paths(workspace_dir).into_iter().collect();
    if all.is_empty() {
        return Vec::new();
    }
    let before = all.len();
    for path in arrived {
        all.remove(path);
    }
    if all.len() != before {
        write_stale_paths(workspace_dir, &all);
    }
    all.into_iter().collect()
}

/// Configures a CMake project into `build/` with `compile_commands.json` before clangd starts,
/// so its background index covers the whole tree (cross-file rename and references) from the
/// first query. Best effort: a failure only means clangd runs without a compilation database.
async fn warm_cmake_compile_commands(workspace_root: &std::path::Path) {
    if !workspace_root.join("CMakeLists.txt").is_file()
        || workspace_root
            .join("build")
            .join("compile_commands.json")
            .is_file()
    {
        return;
    }
    let started = std::time::Instant::now();
    let mut cmd = tokio::process::Command::new("cmake");
    cmd.args([
        "-S",
        ".",
        "-B",
        "build",
        "-DCMAKE_EXPORT_COMPILE_COMMANDS=ON",
    ])
    .current_dir(workspace_root)
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::piped());
    for (k, v) in crate::compiler_cache_env(workspace_root, crate::on_path("ccache")) {
        cmd.env(k, v);
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(180), cmd.output())
        .await;
    match result {
        Ok(Ok(out)) if out.status.success() => tracing::info!(
            workspace = ?workspace_root,
            duration_ms = started.elapsed().as_millis() as u64,
            "cmake configured build/compile_commands.json for clangd"
        ),
        Ok(Ok(out)) => tracing::warn!(
            workspace = ?workspace_root,
            stderr = %String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or(""),
            "cmake configure failed; clangd runs without a compilation database"
        ),
        Ok(Err(e)) => tracing::warn!(workspace = ?workspace_root, error = %e, "cmake not runnable"),
        Err(_) => tracing::warn!(workspace = ?workspace_root, "cmake configure timed out"),
    }
}

/// How long a SwiftPM workspace's load waits for sourcekit-lsp to have the package's build
/// settings. Resolving a package's dependencies the first time can take a while.
const SWIFT_SETTINGS_WAIT: Duration = Duration::from_secs(45);

/// The line a probe appends to a file of the package: a declaration only a type check faults.
const SWIFT_PROBE: &str = "let __prodCodeProbe: Int = \"\"";

/// Holds a SwiftPM workspace's load until sourcekit-lsp checks its files with the package's
/// build settings (#295). Until it has loaded them it checks with fallback settings that report
/// syntax errors only, and the first checks after a load said "0 errors" for code that does not
/// compile. A file of the package, with [`SWIFT_PROBE`] appended, is kept open until the error on
/// that line appears. No session can reach the workspace before its load ends, so nothing else
/// sees the probe. A root without `Package.swift` (an Xcode project) has no settings to wait for.
async fn wait_for_swift_build_settings(
    engine: &prod_code_engine_generic::GenericLspEngine,
    root: &Path,
) {
    if root.components().any(|c| {
        matches!(
            c.as_os_str().to_string_lossy().as_ref(),
            "target"
                | "node_modules"
                | "vendor"
                | "build"
                | "dist"
                | ".build"
                | "Pods"
                | "DerivedData"
        )
    }) {
        return;
    }
    let Some((path, text)) = swift_probe_file(root) else {
        return;
    };
    let (probe, line) = with_probe_line(&text);
    let started = std::time::Instant::now();
    let checked = engine
        .wait_for_semantic_check(&path, "swift", &probe, line, SWIFT_SETTINGS_WAIT)
        .await;
    let waited_ms = started.elapsed().as_millis() as u64;
    match checked {
        Ok(true) => {
            tracing::info!(workspace = ?root, waited_ms, "sourcekit-lsp has the package's build settings")
        }
        Ok(false) => {
            tracing::warn!(workspace = ?root, waited_ms, "sourcekit-lsp found no type error in the probe; its checks may report syntax errors only")
        }
        Err(err) => {
            tracing::warn!(workspace = ?root, waited_ms, error = %err, "sourcekit-lsp never reported on the probe; whether its checks have the package's build settings is unknown")
        }
    }
}

/// A Swift source of the SwiftPM package at `root`, and its text: the first under `Sources/` by
/// path, hidden directories (`.build`) aside.
fn swift_probe_file(root: &Path) -> Option<(PathBuf, String)> {
    if !root.join("Package.swift").is_file() {
        return None;
    }
    let mut pending = vec![root.join("Sources")];
    let mut found = Vec::new();
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "swift") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
        .into_iter()
        .find_map(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
}

/// `text` with [`SWIFT_PROBE`] appended on a line of its own, and that line's 0-based number.
fn with_probe_line(text: &str) -> (String, u64) {
    let mut probe = text.to_string();
    if !probe.is_empty() && !probe.ends_with('\n') {
        probe.push('\n');
    }
    let line = probe.matches('\n').count() as u64;
    probe.push_str(SWIFT_PROBE);
    probe.push('\n');
    (probe, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn generic_workspace(root: &Path) -> (Arc<SharedWorkspace>, PathBuf) {
        let script = root.join("language-server.py");
        std::fs::write(
            &script,
            r#"import json, sys
def read():
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        line = line.strip()
        if not line: break
        if line.lower().startswith(b"content-length:"): length = int(line.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(length)) if length else None
def send(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
while True:
    message = read()
    if message is None: break
    if message.get("method") == "initialize":
        send({"jsonrpc":"2.0", "id":message["id"], "result":{"capabilities":{}}})
"#,
        )
        .unwrap();
        let config = prod_code_engine_generic::GenericLspConfig {
            command: "python3".to_string(),
            args: vec![script.to_string_lossy().into_owned()],
            ..Default::default()
        };
        let engine = prod_code_engine_generic::GenericLspEngine::spawn(root, config)
            .await
            .expect("fake generic engine starts");
        (
            Arc::new(SharedWorkspace::new(
                root.to_path_buf(),
                "python".to_string(),
                None,
                None,
                Some(Arc::new(engine)),
                None,
            )),
            script,
        )
    }

    #[tokio::test]
    async fn generic_validation_capacity_failure_never_falls_back_to_the_main_engine() {
        let dir = tempfile::tempdir().unwrap();
        let (workspace, _) = generic_workspace(dir.path()).await;
        let admission = Arc::new(crate::admission::Admission::with_probe(
            crate::admission::scripted_probe(vec![(10, 100)]),
            2048,
            Duration::ZERO,
        ));
        let err = match workspace.validation_view(&admission).await {
            Ok(_) => panic!("capacity refusal must not return the main engine"),
            Err(err) => err,
        };
        assert!(format!("{err:#}").starts_with("capacity: "), "{err:#}");
        assert!(
            workspace
                .generic_engine
                .as_ref()
                .unwrap()
                .accepts_documents()
        );
    }

    #[tokio::test]
    async fn generic_validation_start_failure_never_falls_back_to_the_main_engine() {
        let dir = tempfile::tempdir().unwrap();
        let (workspace, script) = generic_workspace(dir.path()).await;
        std::fs::remove_file(script).unwrap();
        let err = match workspace
            .validation_view(&Arc::new(crate::admission::Admission::unbounded()))
            .await
        {
            Ok(_) => panic!("start failure must not return the main engine"),
            Err(err) => err,
        };
        assert!(
            format!("{err:#}").contains("private python validation server failed to start"),
            "{err:#}"
        );
        assert!(workspace.generic_engine.as_ref().unwrap().is_alive());
    }

    /// A valid initialized server remains reusable until its output actually exits.
    #[tokio::test]
    async fn a_workspace_whose_server_exited_is_loaded_afresh() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("server.py");
        std::fs::write(&script, r#"import json, sys
while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line == b"\r\n": break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1])
    message = json.loads(sys.stdin.buffer.read(length))
    if message.get("method") == "initialize":
        body = json.dumps({"jsonrpc":"2.0", "id":message["id"], "result":{"capabilities":{}}}).encode()
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        sys.stdout.buffer.flush()
    elif message.get("method") == "initialized" and len(sys.argv) > 1:
        sys.exit(0)
"#).expect("fake server script");
        let workspace = |engine: prod_code_engine_generic::GenericLspEngine| {
            SharedWorkspace::new(
                dir.path().to_path_buf(),
                "typescript".to_string(),
                None,
                None,
                Some(Arc::new(engine)),
                None,
            )
        };
        let config = |exit: bool| {
            let mut args = vec![script.to_string_lossy().into_owned()];
            if exit {
                args.push("exit-after-initialized".to_string());
            }
            prod_code_engine_generic::GenericLspConfig {
                command: "python3".to_string(),
                args,
                ..Default::default()
            }
        };
        let running = workspace(
            prod_code_engine_generic::GenericLspEngine::spawn(dir.path(), config(false))
                .await
                .expect("valid server initializes"),
        );
        let exited = workspace(
            prod_code_engine_generic::GenericLspEngine::spawn(dir.path(), config(true))
                .await
                .expect("short-lived valid server initializes"),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !exited.has_dead_server() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(exited.has_dead_server(), "a server that exited is noticed");
        assert!(
            !exited.reusable_for("typescript"),
            "and its workspace is loaded afresh"
        );
        assert!(!running.has_dead_server(), "a running server is not dead");
        assert!(
            running.reusable_for("typescript"),
            "and its workspace is reused"
        );
        assert!(
            !running.reusable_for("rust"),
            "unless another engine is asked for"
        );
    }

    #[test]
    fn a_swift_probe_is_a_package_source_with_a_type_error_on_its_own_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(
            swift_probe_file(root).is_none(),
            "no Package.swift, nothing to wait for"
        );
        std::fs::write(root.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();
        assert!(swift_probe_file(root).is_none(), "no sources");
        for (path, text) in [
            (
                ".build/checkouts/Dep/Sources/Dep/a.swift",
                "// a dependency\n",
            ),
            ("Sources/Shop/main.swift", "print(1)\n"),
            ("Sources/Shop/Pricing.swift", "func price() -> Int { 1 }"),
            ("Sources/Shop/notes.txt", "not swift\n"),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        }
        let (path, text) = swift_probe_file(root).expect("a source");
        assert!(path.ends_with("Sources/Shop/Pricing.swift"), "{path:?}");
        let (probe, line) = with_probe_line(&text);
        assert_eq!(
            probe,
            "func price() -> Int { 1 }\nlet __prodCodeProbe: Int = \"\"\n"
        );
        assert_eq!(line, 1);
        let (probe, line) = with_probe_line("a\nb\n");
        assert_eq!(probe.lines().nth(line as usize), Some(SWIFT_PROBE));
        assert_eq!(with_probe_line("").1, 0);
    }

    /// A manager admitting against the host memory `snapshots` report in turn, counting every
    /// new engine at 2 GiB.
    fn manager_on(snapshots: Vec<(u64, u64)>) -> Arc<WorkspaceManager> {
        Arc::new(WorkspaceManager::with_admission(Arc::new(
            crate::admission::Admission::with_probe(
                crate::admission::scripted_probe(snapshots),
                2048,
                crate::admission::LOAD_SETTLE,
            ),
        )))
    }

    /// `used` of `total` GiB in use.
    fn host(used: u64, total: u64) -> (u64, u64) {
        const GIB: u64 = 1 << 30;
        ((total - used) * GIB, total * GIB)
    }

    /// A loaded workspace at `root`, without a session for `idle_secs`.
    fn loaded(root: &str, idle_secs: u64) -> Arc<SharedWorkspace> {
        let ws = Arc::new(SharedWorkspace::new(
            PathBuf::from(root),
            "text".to_string(),
            None,
            None,
            None,
            None,
        ));
        ws.last_used
            .store(unix_now() - idle_secs, Ordering::Relaxed);
        ws
    }

    /// Six worktrees handshake at once on a host with 80 of 100 GiB in use: 5 GiB are left
    /// under the 85% limit, so two new engines of 2 GiB load and four are refused for capacity,
    /// while a session of the engine already loaded attaches as before (#433).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn simultaneous_new_engines_load_only_while_memory_lasts() {
        let manager = manager_on(vec![host(80, 100)]);
        manager
            .insert_ready_for_test(loaded("/srv/ws/warm", 0))
            .await;
        let start = Arc::new(tokio::sync::Barrier::new(6));
        let loads: Vec<_> = (0..6)
            .map(|i| {
                let manager = Arc::clone(&manager);
                let start = Arc::clone(&start);
                tokio::spawn(async move {
                    start.wait().await;
                    manager
                        .get_or_load(&PathBuf::from(format!("/srv/ws/new-{i}")), "text")
                        .await
                })
            })
            .collect();
        let mut refusals = Vec::new();
        let mut admitted = 0;
        for load in loads {
            match load.await.unwrap() {
                Ok(_) => admitted += 1,
                Err(err) => refusals.push(err.to_string()),
            }
        }
        assert_eq!(admitted, 2, "{refusals:?}");
        assert_eq!(refusals.len(), 4);
        for refusal in &refusals {
            assert!(refusal.starts_with("capacity: "), "{refusal}");
            assert!(refusal.contains("another node"), "{refusal}");
        }
        assert_eq!(manager.admission().reserved_bytes(), 4 << 30);
        assert_eq!(
            manager.loaded_count().await,
            3,
            "no refused load is left behind"
        );

        let warm = manager
            .get_or_load(Path::new("/srv/ws/warm"), "text")
            .await
            .expect("a loaded engine is used whatever the memory");
        assert_eq!(warm.active_sessions.load(Ordering::Relaxed), 1);
    }

    /// A new engine that finds no room unloads the least recently used engine that has been
    /// idle for a while, as many as it needs, and loads once the host shows the memory back;
    /// engines with a session or used a moment ago stay (#433).
    #[tokio::test]
    async fn a_new_engine_makes_room_by_unloading_only_idle_engines() {
        // 84% in use: a new 2 GiB engine passes the limit by 1 GiB, until one is unloaded.
        let manager = manager_on(vec![host(84, 100), host(82, 100)]);
        let oldest = loaded("/srv/ws/oldest", 7200);
        let older = loaded("/srv/ws/older", 3600);
        let in_use = loaded("/srv/ws/in-use", 9000);
        in_use.active_sessions.store(1, Ordering::Relaxed);
        let recent = loaded("/srv/ws/recent", 30);
        let gone = Arc::downgrade(&oldest);
        for ws in [oldest, older, in_use, recent] {
            manager.insert_ready_for_test(ws).await;
        }

        manager
            .get_or_load(Path::new("/srv/ws/new"), "text")
            .await
            .expect("room was made");
        assert!(!manager.is_loaded(Path::new("/srv/ws/oldest")).await);
        assert!(gone.upgrade().is_none(), "its engine is freed");
        for kept in ["older", "in-use", "recent", "new"] {
            assert!(
                manager.is_loaded(&Path::new("/srv/ws").join(kept)).await,
                "{kept} was unloaded"
            );
        }
    }

    /// With nothing idle long enough to unload, a new engine is refused and the map is left as
    /// it was; so is it when the engines unloaded did not give the memory back.
    #[tokio::test]
    async fn a_new_engine_is_refused_when_unloading_cannot_make_room() {
        let manager = manager_on(vec![host(90, 100)]);
        let in_use = loaded("/srv/ws/in-use", 9000);
        in_use.active_sessions.store(1, Ordering::Relaxed);
        manager.insert_ready_for_test(in_use).await;
        manager
            .insert_ready_for_test(loaded("/srv/ws/recent", 30))
            .await;
        let Err(err) = manager.get_or_load(Path::new("/srv/ws/new"), "text").await else {
            panic!("admitted without room");
        };
        let refused = err
            .downcast_ref::<crate::admission::CapacityRefused>()
            .expect("refused for capacity");
        assert_eq!(refused.reclaimed, 0);
        assert!(!manager.is_loaded(Path::new("/srv/ws/new")).await);
        assert_eq!(manager.loaded_count().await, 2);
        assert_eq!(manager.admission().reserved_bytes(), 0);

        let manager = manager_on(vec![host(90, 100)]);
        manager
            .insert_ready_for_test(loaded("/srv/ws/idle", 7200))
            .await;
        let Err(err) = manager.get_or_load(Path::new("/srv/ws/new"), "text").await else {
            panic!("admitted though unloading gave no memory back");
        };
        let text = err.to_string();
        assert!(text.contains("1 idle engine(s) were unloaded"), "{text}");
        assert_eq!(manager.loaded_count().await, 0);
    }

    /// A Rust loader that reports each load as it starts, holds it until the test lets one
    /// through (or drops the gate), and then fails, so that no real engine is built.
    struct SlowLoader {
        load: RustLoader,
        started: tokio::sync::mpsc::UnboundedReceiver<()>,
        gate: std::sync::mpsc::Sender<()>,
        loads: Arc<AtomicUsize>,
    }

    fn slow_loader() -> SlowLoader {
        let (started_tx, started) = tokio::sync::mpsc::unbounded_channel();
        let (gate, gate_rx) = std::sync::mpsc::channel::<()>();
        let gate_rx = std::sync::Mutex::new(gate_rx);
        let loads = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&loads);
        SlowLoader {
            load: Arc::new(
                move |_root: &Path| -> Result<prod_code_engine_rust::RustEngine> {
                    counted.fetch_add(1, Ordering::SeqCst);
                    let _ = started_tx.send(());
                    let _ = gate_rx.lock().unwrap().recv();
                    anyhow::bail!("scripted load")
                },
            ),
            started,
            gate,
            loads,
        }
    }

    /// Admission on a host with 10 of 100 GiB in use, 2 GiB a new engine, returned as soon as
    /// its load ends.
    fn roomy_admission() -> Arc<crate::admission::Admission> {
        Arc::new(crate::admission::Admission::with_probe(
            crate::admission::scripted_probe(vec![host(10, 100)]),
            2048,
            Duration::ZERO,
        ))
    }

    /// A leader whose client gives up while its Rust engine loads cancels only its wait
    /// (#433): the load goes on holding its reservation, a follower arriving meanwhile is
    /// answered by that same load instead of waiting on a Loading entry forever, and no session
    /// is left counted for the leader that went away.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_canceled_leader_leaves_its_load_running_reserved_and_answering() {
        let SlowLoader {
            load,
            mut started,
            gate,
            loads,
        } = slow_loader();
        let admission = roomy_admission();
        let manager = Arc::new(
            WorkspaceManager::with_admission(Arc::clone(&admission)).with_rust_loader(load),
        );
        let root = PathBuf::from("/srv/ws/slow");
        let attach = || {
            let manager = Arc::clone(&manager);
            let root = root.clone();
            tokio::spawn(async move { manager.get_or_load(&root, "rust").await })
        };

        let leader = attach();
        started.recv().await.expect("the load started");
        leader.abort();
        assert!(leader.await.err().is_some_and(|err| err.is_cancelled()));
        assert_eq!(
            admission.reserved_bytes(),
            2 << 30,
            "the running load keeps its reservation"
        );
        assert!(
            manager.is_loaded(&root).await,
            "the load is still in flight"
        );

        let follower = attach();
        gate.send(()).unwrap();
        let ws = tokio::time::timeout(Duration::from_secs(30), follower)
            .await
            .expect("the follower was answered")
            .unwrap()
            .expect("the load completed");
        assert_eq!(loads.load(Ordering::SeqCst), 1, "one load for both");
        assert_eq!(
            ws.active_sessions.load(Ordering::Relaxed),
            1,
            "only the follower's session is counted"
        );
        assert_eq!(
            admission.reserved_bytes(),
            0,
            "returned once the load ended"
        );
        assert!(manager.get_loaded(&root).await.is_some());
    }

    /// Concurrent cold engine loads across distinct workspaces are bounded by the semaphore (#408):
    /// when the bound is 2, only 2 out of 4 concurrent loads start compilation, and the remaining 2
    /// wait on the semaphore until permits are released.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_engine_loads_are_bounded_by_semaphore() {
        let SlowLoader {
            load,
            mut started,
            gate,
            loads,
        } = slow_loader();
        let manager = Arc::new(
            WorkspaceManager::with_admission(roomy_admission())
                .with_max_concurrent_loads(2)
                .with_rust_loader(load),
        );

        let mut tasks = Vec::new();
        for i in 1..=4 {
            let manager = Arc::clone(&manager);
            let root = PathBuf::from(format!("/srv/ws/bounded-{i}"));
            tasks.push(tokio::spawn(async move {
                manager.get_or_load(&root, "rust").await
            }));
        }

        // Exactly 2 loads acquire the semaphore and start compilation.
        started.recv().await.expect("first load started");
        started.recv().await.expect("second load started");

        // The third and fourth loads are queued and blocked on the semaphore.
        let timeout = tokio::time::timeout(Duration::from_millis(50), started.recv()).await;
        assert!(timeout.is_err(), "third load must wait on semaphore");
        assert_eq!(loads.load(Ordering::SeqCst), 2);

        // Release one permit: third load starts.
        gate.send(()).unwrap();
        started.recv().await.expect("third load started");
        assert_eq!(loads.load(Ordering::SeqCst), 3);

        // Release another permit: fourth load starts.
        gate.send(()).unwrap();
        started.recv().await.expect("fourth load started");
        assert_eq!(loads.load(Ordering::SeqCst), 4);

        // Release the remaining two permits.
        gate.send(()).unwrap();
        gate.send(()).unwrap();

        for task in tasks {
            let res = task.await.unwrap();
            assert!(res.is_ok());
        }
    }

    /// An unloaded in-flight engine cannot replace a later load of the same path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_unloaded_load_cannot_publish_over_its_replacement() {
        let SlowLoader {
            load,
            mut started,
            gate,
            loads,
        } = slow_loader();
        let manager =
            Arc::new(WorkspaceManager::with_admission(roomy_admission()).with_rust_loader(load));
        let root = PathBuf::from("/srv/ws/replaced");
        let attach = || {
            let manager = Arc::clone(&manager);
            let root = root.clone();
            tokio::spawn(async move { manager.get_or_load(&root, "rust").await })
        };
        let old = attach();
        started.recv().await.unwrap();
        assert_eq!(manager.unload_under(&root).await, 1);
        let fresh = attach();
        started.recv().await.unwrap();
        gate.send(()).unwrap();
        let old_result = tokio::time::timeout(Duration::from_secs(30), old)
            .await
            .unwrap()
            .unwrap();
        assert!(
            old_result.is_err(),
            "an unloaded engine was published as ready"
        );
        assert!(
            manager.get_loaded(&root).await.is_none(),
            "the newer load is still pending"
        );
        gate.send(()).unwrap();
        let current = tokio::time::timeout(Duration::from_secs(30), fresh)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let registered = manager.get_loaded(&root).await.unwrap();
        assert!(Arc::ptr_eq(current.workspace(), &registered));
        assert_eq!(current.active_sessions.load(Ordering::Relaxed), 1);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    /// A load that panics, here while it reads the host's memory, is answered like one that
    /// failed: its leader and a follower waiting on it get the error, no Loading entry is left
    /// to trap later sessions, nothing stays reserved, and the next session loads afresh.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_load_that_panics_answers_its_sessions_and_leaves_no_loading_entry() {
        let (started_tx, mut started) = tokio::sync::mpsc::unbounded_channel();
        let (gate, gate_rx) = std::sync::mpsc::channel::<()>();
        let gate_rx = std::sync::Mutex::new(gate_rx);
        let reads = AtomicUsize::new(0);
        let probe: crate::admission::MemoryProbe = Arc::new(move || {
            if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                let _ = started_tx.send(());
                let _ = gate_rx.lock().unwrap().recv();
                panic!("scripted probe failure");
            }
            Some(host(10, 100))
        });
        let manager = Arc::new(WorkspaceManager::with_admission(Arc::new(
            crate::admission::Admission::with_probe(probe, 2048, Duration::ZERO),
        )));
        let root = PathBuf::from("/srv/ws/panics");
        let attach = || {
            let manager = Arc::clone(&manager);
            let root = root.clone();
            tokio::spawn(async move { manager.get_or_load(&root, "text").await })
        };

        let leader = attach();
        started.recv().await.expect("the load started");
        let follower = attach();
        // The leader holds one receiver of the load's broadcast; the follower adds another.
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let subscribed = match manager
                    .workspaces
                    .read()
                    .await
                    .get(&WorkspaceKey(root.clone()))
                {
                    Some(LoadState::Loading(tx)) => tx.receiver_count() >= 2,
                    _ => false,
                };
                if subscribed {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the follower waits on the load");
        gate.send(()).unwrap();

        let answered = |task: tokio::task::JoinHandle<Result<WorkspaceLease>>| async {
            tokio::time::timeout(Duration::from_secs(30), task)
                .await
                .expect("answered")
                .unwrap()
                .err()
                .map(|err| format!("{err:#}"))
        };
        let leader_err = answered(leader)
            .await
            .expect("the leader is told it failed");
        assert!(leader_err.contains("panicked"), "{leader_err}");
        let follower_err = answered(follower)
            .await
            .expect("the follower is told it failed");
        assert!(follower_err.contains("panicked"), "{follower_err}");
        assert!(
            !manager.is_loaded(&root).await,
            "no Loading entry is left behind"
        );
        assert_eq!(manager.admission().reserved_bytes(), 0);

        let ws = manager
            .get_or_load(&root, "text")
            .await
            .expect("loaded afresh");
        assert_eq!(ws.active_sessions.load(Ordering::Relaxed), 1);
    }

    /// The validation engine is a load like any other: a session that stops waiting leaves it
    /// running with its reservation, and the next session waits for that load instead of
    /// starting a second one beside it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_canceled_validation_session_leaves_one_load_running_with_its_reservation() {
        let SlowLoader {
            load,
            mut started,
            gate,
            loads,
        } = slow_loader();
        let admission = roomy_admission();
        let ws = loaded("/srv/ws/validated", 0);
        let validate = || {
            let ws = Arc::clone(&ws);
            let admission = Arc::clone(&admission);
            let load = Arc::clone(&load);
            tokio::spawn(async move { ws.validation_engine(&admission, load).await.is_some() })
        };

        let first = validate();
        started.recv().await.expect("the load started");
        first.abort();
        assert!(first.await.err().is_some_and(|err| err.is_cancelled()));
        assert_eq!(
            admission.reserved_bytes(),
            2 << 30,
            "the running load keeps its reservation"
        );

        let second = validate();
        gate.send(()).unwrap();
        let validated = tokio::time::timeout(Duration::from_secs(30), second)
            .await
            .expect("the second session was answered")
            .unwrap();
        assert!(
            !validated,
            "the scripted load fails: validation stays on the main engine"
        );
        assert_eq!(loads.load(Ordering::SeqCst), 1, "one load for both");
        assert_eq!(
            admission.reserved_bytes(),
            0,
            "returned once the load ended"
        );
    }

    /// A validation engine the host has no memory for is not loaded, and not remembered as
    /// failed: a later session, once the memory is back, loads it.
    #[tokio::test]
    async fn a_refused_validation_engine_is_asked_for_again() {
        let SlowLoader {
            load, gate, loads, ..
        } = slow_loader();
        drop(gate);
        let admission = Arc::new(crate::admission::Admission::with_probe(
            crate::admission::scripted_probe(vec![host(90, 100), host(10, 100)]),
            2048,
            Duration::ZERO,
        ));
        let ws = loaded("/srv/ws/validated", 0);
        assert!(
            ws.validation_engine(&admission, Arc::clone(&load))
                .await
                .is_none()
        );
        assert_eq!(loads.load(Ordering::SeqCst), 0, "refused before loading");
        assert!(
            ws.validation_engine(&admission, Arc::clone(&load))
                .await
                .is_none()
        );
        assert_eq!(
            loads.load(Ordering::SeqCst),
            1,
            "asked again once memory is back"
        );
        assert_eq!(admission.reserved_bytes(), 0);
    }

    #[tokio::test]
    async fn test_leader_follower_coalescing() {
        let manager = Arc::new(WorkspaceManager::with_admission(Arc::new(
            crate::admission::Admission::unbounded(),
        )));
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
        assert!(Arc::ptr_eq(ws1.workspace(), ws2.workspace()));
        assert_eq!(manager.loaded_count().await, 1);
        assert_eq!(ws1.active_sessions.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn test_single_owner_detection() {
        let manager = Arc::new(WorkspaceManager::with_admission(Arc::new(
            crate::admission::Admission::unbounded(),
        )));
        let root = PathBuf::from("/test/repo");
        let wt1 = PathBuf::from("/test/repo/worktree-1");
        let wt2 = PathBuf::from("/test/repo/worktree-2");

        let view1 = manager
            .register_session_view(
                1,
                wt1.clone(),
                manager.get_or_load(&root, "rust").await.unwrap(),
            )
            .await;
        assert!(view1.is_single_owner(), "First agent on wt1 is sole owner");

        let view2 = manager
            .register_session_view(
                2,
                wt2.clone(),
                manager.get_or_load(&root, "rust").await.unwrap(),
            )
            .await;
        assert!(view2.is_single_owner(), "First agent on wt2 is sole owner");

        // Second session attaches to wt1
        let view3 = manager
            .register_session_view(
                3,
                wt1.clone(),
                manager.get_or_load(&root, "rust").await.unwrap(),
            )
            .await;
        assert!(
            !view3.is_single_owner(),
            "Second agent on wt1 is NOT sole owner"
        );
        assert!(
            !view1.is_single_owner(),
            "First agent on wt1 direct-edit exclusivity was revoked when second session joined"
        );

        // Cleanup
        manager.unregister_session_view(view1).await;
        manager.unregister_session_view(view2).await;
        manager.unregister_session_view(view3).await;
    }

    #[tokio::test]
    async fn test_single_owner_direct_edit_fast_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let src_dir = root.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let cargo_toml = root.join("Cargo.toml");
        std::fs::write(
            &cargo_toml,
            r#"[package]
name = "fast_path_fixture"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
        )
        .unwrap();
        let lib_path = src_dir.join("lib.rs");
        std::fs::write(&lib_path, "pub const BASE_VAL: u32 = 100;\n").unwrap();

        let engine = prod_code_engine_rust::RustEngine::load(&root).expect("load engine");
        let engine_arc = Arc::new(Mutex::new(engine));
        let ws = Arc::new(SharedWorkspace::new(
            root.clone(),
            "rust".to_string(),
            Some(Arc::clone(&engine_arc)),
            None,
            None,
            None,
        ));

        let manager = Arc::new(WorkspaceManager::with_admission(Arc::new(
            crate::admission::Admission::unbounded(),
        )));
        manager.insert_ready_for_test(Arc::clone(&ws)).await;

        let lease1 = manager.get_or_load(&root, "rust").await.unwrap();
        let view1 = manager.register_session_view(101, root.clone(), lease1).await;
        assert!(view1.is_single_owner(), "Dedicated worktree is single owner");

        let direct_text = "pub const BASE_VAL: u32 = 100;\npub fn direct_added() {}\n".to_string();
        // Fast path: direct edit modifies base Salsa input without session overlays
        {
            let mut eng = engine_arc.lock().await;
            assert!(!eng.has_session_overlays());
            eng.apply_file_change(&lib_path, direct_text.clone()).unwrap();
            assert!(
                !eng.has_session_overlays(),
                "Direct edits must not create session overlays"
            );
            assert_eq!(eng.session_overlay_count(view1.session_id), 0);

            let syms = eng.document_symbols(&lib_path).unwrap();
            assert!(syms.iter().any(|s| s.name == "direct_added"));
        }

        // Track open file in view1
        view1
            .direct_edit_open_files
            .lock()
            .unwrap()
            .insert(lib_path.clone(), direct_text);

        // When a second session joins the same worktree, direct-edit exclusivity must be revoked
        // and view1's direct edits migrated into view1's session overlay in the engine!
        let lease2 = manager.get_or_load(&root, "rust").await.unwrap();
        let view2 = manager.register_session_view(102, root.clone(), lease2).await;
        assert!(!view1.is_single_owner(), "view1 exclusivity must be revoked when view2 joins");
        assert!(!view2.is_single_owner(), "view2 must not have single-owner exclusivity");

        {
            let mut eng = engine_arc.lock().await;
            // The engine now has session overlays for view1
            assert!(eng.has_session_overlays());
            assert_eq!(eng.session_overlay_count(view1.session_id), 1);
            assert_eq!(eng.session_overlay_count(view2.session_id), 0);

            // Base Salsa DB was restored from disk!
            eng.activate_session(view2.session_id).unwrap();
            let view2_syms = eng.document_symbols(&lib_path).unwrap();
            assert!(
                !view2_syms.iter().any(|s| s.name == "direct_added"),
                "view2 must NOT observe view1's unsaved direct edits in base Salsa DB!"
            );

            // view1's unsaved edits are preserved in its session overlay!
            eng.activate_session(view1.session_id).unwrap();
            let view1_syms = eng.document_symbols(&lib_path).unwrap();
            assert!(
                view1_syms.iter().any(|s| s.name == "direct_added"),
                "view1 must observe its unsaved edits in its session overlay"
            );
        }

        // Unregister both sessions
        manager.unregister_session_view(view1).await;
        manager.unregister_session_view(view2).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        {
            let eng = engine_arc.lock().await;
            assert!(!eng.has_session_overlays());
            let syms = eng.document_symbols(&lib_path).unwrap();
            assert!(
                !syms.iter().any(|s| s.name == "direct_added"),
                "Disk state must be restored after both sessions retire"
            );
        }
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

    /// A copy's name never begins with `.` or `_`, which Go tools skip as hidden (#391).
    #[test]
    fn a_copy_is_never_named_as_a_hidden_directory() {
        assert_eq!(sanitize_identifier(".tmpZAcUGt"), "dot-tmpZAcUGt");
        assert_eq!(sanitize_identifier("_scratch"), "under-scratch");
        assert_eq!(sanitize_identifier("prod.codes"), "prod.codes");
        assert_eq!(sanitize_identifier("my repo"), "my_repo");
        assert_eq!(sanitize_identifier(""), "workspace");
    }

    /// Low on space, idle worktree copies go oldest first until enough is free, however young;
    /// one used within the hour and the main copy stay (#386).
    #[tokio::test]
    async fn worktree_copies_are_pruned_oldest_first_when_space_runs_low() {
        let temp = tempfile::tempdir().unwrap();
        let storage = temp.path();
        let manager = WorkspaceManager::new();
        let aged = |name: &str, hours: u64| {
            let dir = storage.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            touch_last_used(&dir);
            std::fs::File::options()
                .write(true)
                .open(dir.join(LAST_USED_MARKER))
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(hours * 3600))
                .unwrap();
            dir
        };
        let oldest = aged("repo--wt-00000001", 44);
        let older = aged("repo--wt-00000002", 30);
        let old = aged("repo--wt-00000003", 25);
        let in_use = aged("repo--wt-00000004", 0);
        let main = aged("repo", 100);
        // Every copy removed frees ten points: 5% free with four copies, 25% with two left.
        let copies = |root: &Path| {
            std::fs::read_dir(root)
                .unwrap()
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().contains("--wt-"))
                .count() as f64
        };
        let free = |root: &Path| Some(0.05 + 0.10 * (4.0 - copies(root)));

        let removed = prune_worktree_dirs_for_space(storage, 0.20, &manager, free).await;
        assert_eq!(removed, vec![oldest.clone(), older.clone()]);
        assert!(old.exists() && in_use.exists() && main.exists());

        // With enough free, nothing goes.
        let plenty = |_: &Path| Some(0.50);
        assert!(
            prune_worktree_dirs_for_space(storage, 0.20, &manager, plenty)
                .await
                .is_empty()
        );
        // Out of candidates, the one in use still stays.
        let full = |_: &Path| Some(0.0);
        let rest = prune_worktree_dirs_for_space(storage, 0.20, &manager, full).await;
        assert_eq!(rest, vec![old.clone()]);
        assert!(in_use.exists() && main.exists());
        assert!(free_share(storage).is_some_and(|share| (0.0..=1.0).contains(&share)));
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

    #[tokio::test]
    async fn test_prune_stale_main_workspace_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let storage = temp.path();
        let manager = WorkspaceManager::new();

        let old_main = storage.join("repo-old");
        let recent_main = storage.join("repo-recent");
        let hidden = storage.join(".prod-code-shadow");
        let lost_found = storage.join("lost+found");
        let wt = storage.join("repo-old--wt-12345678");

        for d in [&old_main, &recent_main, &hidden, &lost_found, &wt] {
            std::fs::create_dir_all(d).unwrap();
            touch_last_used(d);
        }

        let now = SystemTime::now();
        let two_days_ago = now - Duration::from_secs(2 * 86_400);
        let ten_minutes_ago = now - Duration::from_secs(600);

        // old_main is 2 days old
        touch_last_used_at(&old_main, unix_now().saturating_sub(2 * 86_400));
        let _ = std::fs::File::options()
            .write(true)
            .open(old_main.join(LAST_USED_MARKER))
            .unwrap()
            .set_modified(two_days_ago);

        // recent_main is 10 minutes old
        touch_last_used_at(&recent_main, unix_now().saturating_sub(600));
        let _ = std::fs::File::options()
            .write(true)
            .open(recent_main.join(LAST_USED_MARKER))
            .unwrap()
            .set_modified(ten_minutes_ago);

        // hidden and lost+found are also old
        let _ = std::fs::File::options()
            .write(true)
            .open(hidden.join(LAST_USED_MARKER))
            .unwrap()
            .set_modified(two_days_ago);
        let _ = std::fs::File::options()
            .write(true)
            .open(lost_found.join(LAST_USED_MARKER))
            .unwrap()
            .set_modified(two_days_ago);

        // wt is also old, but prune_stale_main_workspace_dirs ignores worktrees
        let _ = std::fs::File::options()
            .write(true)
            .open(wt.join(LAST_USED_MARKER))
            .unwrap()
            .set_modified(two_days_ago);

        // Pruning with 1-day (86400s) timeout
        let removed = prune_stale_main_workspace_dirs(
            storage,
            Duration::from_secs(86_400),
            Duration::from_secs(3600),
            &manager,
        )
        .await;

        assert_eq!(removed, vec![old_main.clone()]);
        assert!(!old_main.exists());
        assert!(recent_main.exists());
        assert!(hidden.exists());
        assert!(lost_found.exists());
        assert!(wt.exists()); // not pruned by main workspace pruner
    }

    #[tokio::test]
    async fn test_prune_stale_main_workspace_protected_by_active_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let storage = temp.path();
        let manager = WorkspaceManager::new();

        let main = storage.join("active-project");
        let active_wt = storage.join("active-project--wt-abcdef12");

        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&active_wt).unwrap();
        touch_last_used(&main);
        touch_last_used(&active_wt);

        let now = SystemTime::now();
        let two_days_ago = now - Duration::from_secs(2 * 86_400);

        // Main is old (2 days ago)
        let _ = std::fs::File::options()
            .write(true)
            .open(main.join(LAST_USED_MARKER))
            .unwrap()
            .set_modified(two_days_ago);

        // Active worktree is fresh (just touched)
        // Main should be protected because active_wt is idle < 3600s
        let removed = prune_stale_main_workspace_dirs(
            storage,
            Duration::from_secs(86_400),
            Duration::from_secs(3600),
            &manager,
        )
        .await;
        assert!(removed.is_empty());
        assert!(main.exists());
        assert!(active_wt.exists());

        // Now age the worktree to 2 hours ago
        let two_hours_ago = now - Duration::from_secs(2 * 3600);
        let _ = std::fs::File::options()
            .write(true)
            .open(active_wt.join(LAST_USED_MARKER))
            .unwrap()
            .set_modified(two_hours_ago);

        // First worktree pruner cleans up stale worktree
        let wt_removed =
            prune_stale_worktree_dirs(storage, Duration::from_secs(3600), &manager).await;
        assert_eq!(wt_removed, vec![active_wt.clone()]);
        assert!(!active_wt.exists());

        // Now main workspace has no active worktrees, and can be pruned
        let main_removed = prune_stale_main_workspace_dirs(
            storage,
            Duration::from_secs(86_400),
            Duration::from_secs(3600),
            &manager,
        )
        .await;
        assert_eq!(main_removed, vec![main.clone()]);
        assert!(!main.exists());
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

    #[test]
    fn test_split_worktree_base() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = temp_dir.path();
        let base_dir = storage.join("my-service");
        std::fs::create_dir_all(&base_dir).unwrap();

        let wt_dir = storage.join("my-service--wt-a1b2c3d4");
        std::fs::create_dir_all(&wt_dir).unwrap();
        assert_eq!(split_worktree_base(&wt_dir), Some(base_dir.clone()));

        let nested_wt = wt_dir.join("crates").join("sub-crate");
        assert_eq!(
            split_worktree_base(&nested_wt),
            Some(base_dir.join("crates").join("sub-crate"))
        );

        assert_eq!(split_worktree_base(&base_dir), None);

        let non_existent_base = storage.join("other--wt-12345678");
        assert_eq!(split_worktree_base(&non_existent_base), None);

        // Test standard git worktree with `.git` file pointing to base repo
        let git_base = storage.join("git-repo");
        let git_dir = git_base.join(".git");
        let wt_meta = git_dir.join("worktrees").join("branch-wt");
        std::fs::create_dir_all(&wt_meta).unwrap();

        let git_wt = storage.join("git-repo-wt");
        std::fs::create_dir_all(&git_wt).unwrap();
        std::fs::write(
            git_wt.join(".git"),
            format!("gitdir: {}\n", wt_meta.display()),
        )
        .unwrap();

        assert_eq!(split_worktree_base(&git_wt), Some(git_base.clone()));

        // Test nested package inside standard git worktree (no local .git file, found in ancestor)
        let nested_git_wt = git_wt.join("crates").join("sub-crate");
        let nested_git_base = git_base.join("crates").join("sub-crate");
        std::fs::create_dir_all(&nested_git_wt).unwrap();
        std::fs::create_dir_all(&nested_git_base).unwrap();
        assert_eq!(split_worktree_base(&nested_git_wt), Some(nested_git_base));
    }

    #[tokio::test]
    async fn test_worktree_shares_base_rust_engine() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = temp_dir.path();
        let base_dir = storage.join("sample-repo");
        std::fs::create_dir_all(base_dir.join("src")).unwrap();
        std::fs::write(
            base_dir.join("Cargo.toml"),
            "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            base_dir.join("src/lib.rs"),
            "pub fn base_func() -> u32 { 42 }\n",
        )
        .unwrap();

        let wt_dir = storage.join(format!("sample-repo{}", worktree_suffix("client-wt-path")));
        std::fs::create_dir_all(wt_dir.join("src")).unwrap();
        std::fs::write(
            wt_dir.join("Cargo.toml"),
            "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            wt_dir.join("src/lib.rs"),
            "pub fn base_func() -> u32 { 100 }\n",
        )
        .unwrap();

        let manager = Arc::new(WorkspaceManager::new());

        let base_lease = manager.get_or_load(&base_dir, "rust").await.unwrap();
        let base_ws = Arc::clone(base_lease.workspace());
        assert!(base_lease.rust_engine.is_some());
        assert!(base_lease.base_workspace.is_none());

        let wt_lease = manager.get_or_load(&wt_dir, "rust").await.unwrap();
        assert!(wt_lease.rust_engine.is_some());
        assert!(wt_lease.base_workspace.is_some());

        // Same underlying Arc<Mutex<RustEngine>>
        let base_eng = base_lease.rust_engine.as_ref().unwrap();
        let wt_eng = wt_lease.rust_engine.as_ref().unwrap();
        assert!(Arc::ptr_eq(base_eng, wt_eng));

        // Engine has worktree attached
        assert!(base_eng.lock().await.has_worktree(&wt_dir));

        // Worktree does not duplicate memory reclaimable
        assert_eq!(wt_lease.reclaimable(manager.admission()), 0);

        // Attached worktree increments base's attached_worktrees count
        assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 1);
        // Base is not reclaimable while attached worktree exists
        assert_eq!(base_ws.reclaimable(manager.admission()), 0);

        // Validation view forwards to base validation and keeps worktree attached
        let wt_val = wt_lease.workspace().validation_view(manager.admission()).await.unwrap();
        assert!(wt_val.base_workspace.is_some());
        assert!(Arc::ptr_eq(
            wt_val.base_workspace.as_ref().unwrap(),
            base_lease.workspace()
        ));

        // Active worktree lease pins base against eviction even if base_lease drops
        drop(base_lease);
        assert_eq!(base_ws.active_sessions.load(Ordering::Relaxed), 1);
        let early_evict = manager.evict_idle(Duration::from_secs(0)).await;
        assert!(early_evict.is_empty(), "base must not be evicted while worktree is active");

        // Dropping worktree lease frees sessions, but attached_worktrees protects base until worktree is evicted
        drop(wt_lease);
        assert_eq!(base_ws.active_sessions.load(Ordering::Relaxed), 0);
        assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 1);

        // Evicting idle worktrees detaches overlay and frees base
        let evicted = manager.evict_idle(Duration::from_secs(0)).await;
        assert!(evicted.contains(&wt_dir));
        assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 0);

        // Now base has 0 attached worktrees and can be evicted
        let evicted_base = manager.evict_idle(Duration::from_secs(0)).await;
        assert!(evicted_base.contains(&base_dir));
    }

    #[tokio::test]
    async fn test_worktree_overlay_preserved_during_unload_while_active_lease_exists() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = temp_dir.path();
        let base_dir = storage.join("sample-repo");
        std::fs::create_dir_all(base_dir.join("src")).unwrap();
        std::fs::write(
            base_dir.join("Cargo.toml"),
            "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            base_dir.join("src/lib.rs"),
            "pub fn base_func() -> u32 { 42 }\n",
        )
        .unwrap();

        let wt_dir = storage.join(format!("sample-repo{}", worktree_suffix("client-wt-path")));
        std::fs::create_dir_all(wt_dir.join("src")).unwrap();
        std::fs::write(
            wt_dir.join("Cargo.toml"),
            "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            wt_dir.join("src/lib.rs"),
            "pub fn base_func() -> u32 { 100 }\n",
        )
        .unwrap();

        let manager = Arc::new(WorkspaceManager::new());
        let _base_lease = manager.get_or_load(&base_dir, "rust").await.unwrap();
        let wt_lease = manager.get_or_load(&wt_dir, "rust").await.unwrap();
        let base_ws = Arc::clone(wt_lease.base_workspace.as_ref().unwrap());
        let base_eng = Arc::clone(base_ws.rust_engine.as_ref().unwrap());

        // Overlay is attached and base worktree count is 1
        assert!(base_eng.lock().await.has_worktree(&wt_dir));
        assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 1);
        assert_eq!(wt_lease.workspace().active_sessions.load(Ordering::Relaxed), 1);

        // Unload the worktree workspace while lease is still held (e.g. manifest change during session)
        let unloaded = manager.unload_under(&wt_dir).await;
        assert_eq!(unloaded, 1);
        assert!(!manager.is_loaded(&wt_dir).await);

        // CRITICAL: Overlay MUST remain attached and base pin preserved for extant lease
        assert!(
            base_eng.lock().await.has_worktree(&wt_dir),
            "overlay must remain attached while lease is active even after unload_under"
        );
        assert_eq!(
            base_ws.attached_worktrees.load(Ordering::Relaxed),
            1,
            "base attached_worktrees must not decrement while lease is active"
        );

        // Dropping the active lease triggers final detachment
        drop(wt_lease);

        // Wait briefly for the detachment background task if spawned
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !base_eng.lock().await.has_worktree(&wt_dir)
                    && base_ws.attached_worktrees.load(Ordering::Relaxed) == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("overlay detachment after final lease drop");

        assert!(!base_eng.lock().await.has_worktree(&wt_dir));
        assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_repeated_validation_views_do_not_leak_validation_engine_attachment_refcount() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = temp_dir.path();
        let base_dir = storage.join("sample-repo");
        std::fs::create_dir_all(base_dir.join("src")).unwrap();
        std::fs::write(
            base_dir.join("Cargo.toml"),
            "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            base_dir.join("src/lib.rs"),
            "pub fn base_func() -> u32 { 42 }\n",
        )
        .unwrap();

        let wt_dir = storage.join(format!("sample-repo{}", worktree_suffix("client-wt-path")));
        std::fs::create_dir_all(wt_dir.join("src")).unwrap();
        std::fs::write(
            wt_dir.join("Cargo.toml"),
            "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            wt_dir.join("src/lib.rs"),
            "pub fn base_func() -> u32 { 100 }\n",
        )
        .unwrap();

        let manager = Arc::new(WorkspaceManager::new());
        let _base_lease = manager.get_or_load(&base_dir, "rust").await.unwrap();
        let wt_lease = manager.get_or_load(&wt_dir, "rust").await.unwrap();
        let wt_ws = wt_lease.workspace();
        let base_ws = Arc::clone(wt_ws.base_workspace.as_ref().unwrap());

        let admission = Arc::new(crate::admission::Admission::unbounded());

        // Repeatedly request validation views on the worktree
        let mut val_views = Vec::new();
        for _ in 0..5 {
            let val_ws = wt_ws.validation_view(&admission).await.unwrap();
            val_views.push(val_ws);
        }

        // Get the validation engine from the base workspace
        let val_eng_arc = base_ws
            .validation
            .get()
            .and_then(|opt| opt.as_ref())
            .expect("validation engine must be initialized");

        {
            let val_eng = val_eng_arc.lock().await;
            assert!(val_eng.has_worktree(&wt_dir));
            assert_eq!(
                val_eng.worktree_attachment_count(&wt_dir),
                1,
                "validation engine attachment count must be 1 regardless of repeated validation views"
            );
        }

        // Drop transient validation views
        drop(val_views);

        // Overlay remains attached while worktree workspace is alive
        assert!(val_eng_arc.lock().await.has_worktree(&wt_dir));

        // Unload and drop lease to trigger detachment
        drop(wt_lease);
        let unloaded = manager.unload_under(&wt_dir).await;
        assert_eq!(unloaded, 1);

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !val_eng_arc.lock().await.has_worktree(&wt_dir) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("validation engine overlay detachment after worktree unload");

        assert!(
            !val_eng_arc.lock().await.has_worktree(&wt_dir),
            "validation engine overlay must be detached after worktree is unloaded and retired"
        );
        assert_eq!(
            val_eng_arc.lock().await.worktree_attachment_count(&wt_dir),
            0
        );
    }

    #[tokio::test]
    async fn test_validation_fallback_does_not_leak_main_engine_attachment() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = temp_dir.path();
        let base_dir = storage.join("sample-repo");
        std::fs::create_dir_all(base_dir.join("src")).unwrap();
        std::fs::write(
            base_dir.join("Cargo.toml"),
            "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            base_dir.join("src/lib.rs"),
            "pub fn base_func() -> u32 { 42 }\n",
        )
        .unwrap();

        let wt_dir = storage.join(format!("sample-repo{}", worktree_suffix("client-wt-path")));
        std::fs::create_dir_all(wt_dir.join("src")).unwrap();
        std::fs::write(
            wt_dir.join("Cargo.toml"),
            "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            wt_dir.join("src/lib.rs"),
            "pub fn base_func() -> u32 { 100 }\n",
        )
        .unwrap();

        let manager = Arc::new(WorkspaceManager::new());
        let _base_lease = manager.get_or_load(&base_dir, "rust").await.unwrap();
        let wt_lease = manager.get_or_load(&wt_dir, "rust").await.unwrap();
        let wt_ws = wt_lease.workspace();
        let main_eng_arc = Arc::clone(wt_ws.rust_engine.as_ref().unwrap());

        // Base worktree attachment count is 1 initially
        {
            let eng = main_eng_arc.lock().await;
            assert!(eng.has_worktree(&wt_dir));
            assert_eq!(eng.worktree_attachment_count(&wt_dir), 1);
        }

        // Host has 95 of 100 GiB used, so admission refuses a second validation engine
        let refused_admission = Arc::new(crate::admission::Admission::with_probe(
            crate::admission::scripted_probe(vec![host(95, 100)]),
            2048,
            Duration::ZERO,
        ));

        // Call validation_view when admission has no capacity: falls back to main engine
        let val_ws = wt_ws.validation_view(&refused_admission).await.unwrap();
        assert!(Arc::ptr_eq(
            val_ws.rust_engine.as_ref().unwrap(),
            &main_eng_arc
        ));

        // CRITICAL: The main engine attachment count must still be 1 (NOT 2)
        {
            let eng = main_eng_arc.lock().await;
            assert_eq!(
                eng.worktree_attachment_count(&wt_dir),
                1,
                "validation fallback must not acquire a second attachment on the main engine"
            );
        }

        drop(val_ws);
        drop(wt_lease);

        // Unload the worktree
        let unloaded = manager.unload_under(&wt_dir).await;
        assert_eq!(unloaded, 1);

        // Detach background task
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !main_eng_arc.lock().await.has_worktree(&wt_dir) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("main engine overlay detachment after unload");

        // The overlay must be completely gone and refcount 0
        {
            let eng = main_eng_arc.lock().await;
            assert!(!eng.has_worktree(&wt_dir));
            assert_eq!(eng.worktree_attachment_count(&wt_dir), 0);
        }
    }

    #[tokio::test]
    async fn test_trigger_rebalance_exact_name_matching_and_worktree() {
        let manager = WorkspaceManager::new();
        let path_shop = PathBuf::from("/work/shop");
        let path_shopper = PathBuf::from("/work/shopper");
        let path_shop_wt = PathBuf::from("/work/shop--wt1");

        let ws_shop = Arc::new(SharedWorkspace::new(
            path_shop.clone(),
            "rust".to_string(),
            None,
            None,
            None,
            None,
        ));
        let ws_shopper = Arc::new(SharedWorkspace::new(
            path_shopper.clone(),
            "rust".to_string(),
            None,
            None,
            None,
            None,
        ));
        let ws_shop_wt = Arc::new(SharedWorkspace::with_base(
            path_shop_wt.clone(),
            "rust".to_string(),
            None,
            None,
            None,
            None,
            Some(Arc::clone(&ws_shop)),
        ));

        let mut rx_shop = ws_shop.subscribe_rebalance();
        let mut rx_shopper = ws_shopper.subscribe_rebalance();
        let mut rx_shop_wt = ws_shop_wt.subscribe_rebalance();

        {
            let mut guard = manager.workspaces.write().await;
            guard.insert(WorkspaceKey(path_shop), LoadState::Ready(Arc::clone(&ws_shop)));
            guard.insert(WorkspaceKey(path_shopper), LoadState::Ready(Arc::clone(&ws_shopper)));
            guard.insert(WorkspaceKey(path_shop_wt), LoadState::Ready(Arc::clone(&ws_shop_wt)));
        }

        // Rebalance "shop" to node-2:2026
        let notified = manager
            .trigger_rebalance_by_name("shop", "node-2:2026".to_string(), Some("test".to_string()))
            .await;
        assert!(notified >= 1);

        // ws_shop and ws_shop_wt share the rebalance broadcast channel, so both receive redirect
        let (target, reason) = rx_shop.try_recv().expect("shop must receive redirect");
        assert_eq!(target, "node-2:2026");
        assert_eq!(reason.as_deref(), Some("test"));

        let (target_wt, reason_wt) = rx_shop_wt.try_recv().expect("shop worktree must receive redirect");
        assert_eq!(target_wt, "node-2:2026");
        assert_eq!(reason_wt.as_deref(), Some("test"));

        // ws_shopper must NOT receive redirect (preventing substring false positive)
        assert!(
            rx_shopper.try_recv().is_err(),
            "shopper must NOT receive redirect when rebalancing shop"
        );
    }
}
