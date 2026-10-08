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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use tokio::sync::{Mutex, broadcast};

use super::types::{
    RustEngines, WatchedChange, WorkspaceKey, default_max_concurrent_engine_loads, unix_now,
};

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
    pub loaded_at: Instant,
    pub direct_edit_eligible: AtomicBool,
    pub rust_engine: Option<Arc<Mutex<prod_code_engine_rust::RustEngine>>>,
    pub go_engine: Option<Arc<prod_code_engine_go::GoEngine>>,
    pub generic_engine: Option<Arc<prod_code_engine_generic::GenericLspEngine>>,
    pub backend: Option<Arc<crate::backend::BackendWorker>>,
    /// All the Rust engines for this root, `rust_engine` first; see [`RustEngines`].
    pub rust_engines: RustEngines,
    /// The second engine validation sessions run on, loaded by the first of them (#73).
    /// `None` inside once a load failed: validation then falls back to the main engine.
    pub(crate) validation:
        tokio::sync::OnceCell<Option<Arc<Mutex<prod_code_engine_rust::RustEngine>>>>,
    /// The second generic server validation sessions run on, started by the first of them.
    /// A full retained-document generation is dropped and replaced as one unit.
    pub(crate) generic_validation: Mutex<Option<Arc<prod_code_engine_generic::GenericLspEngine>>>,
    pub(crate) generic_validation_loaded: AtomicBool,
    /// A generic server has one process-wide document state. Validation sessions use it one
    /// at a time so parallel proposals cannot replace each other's overlays.
    pub generic_validation_session: Arc<tokio::sync::Mutex<()>>,
    /// Shared with the manager's primary loads so private validation servers use the same
    /// node-wide concurrency budget.
    pub(crate) engine_load_semaphore: Arc<tokio::sync::Semaphore>,
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
    /// Recorded modification timestamps for project manifests when loaded.
    pub manifest_mtimes: HashMap<PathBuf, Option<std::time::SystemTime>>,
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
        Self::with_base(
            root,
            engine,
            rust_engine,
            go_engine,
            generic_engine,
            backend,
            None,
        )
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
            .unwrap_or_else(|| {
                Arc::new(std::sync::Mutex::new(rust_engine.iter().cloned().collect()))
            });
        let (rebalance_tx, _) = broadcast::channel(16);
        let rebalance_tx = base_workspace
            .as_ref()
            .map(|b| b.rebalance_tx.clone())
            .unwrap_or(rebalance_tx);
        let engine_load_semaphore = base_workspace
            .as_ref()
            .map(|base| Arc::clone(&base.engine_load_semaphore))
            .unwrap_or_else(|| {
                Arc::new(tokio::sync::Semaphore::new(
                    default_max_concurrent_engine_loads(),
                ))
            });
        let mut manifest_mtimes = HashMap::new();
        for cfg in [
            "Cargo.lock",
            "Cargo.toml",
            "go.mod",
            "go.work",
            "go.sum",
            "package.json",
            "tsconfig.json",
            "pyproject.toml",
            "setup.py",
            "uv.lock",
        ] {
            let p = root.join(cfg);
            let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            manifest_mtimes.insert(p, mtime);
        }
        Self {
            key: WorkspaceKey(root.clone()),
            root,
            engine,
            active_sessions: AtomicUsize::new(0),
            last_used: AtomicU64::new(unix_now()),
            loaded_at: Instant::now(),
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
            manifest_mtimes,
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

    pub fn touch(&self) {
        self.last_used.store(unix_now(), Ordering::Relaxed);
    }

    /// What unloading this workspace is counted to free: its engine, and its validation engine
    /// once one runs.
    pub(crate) fn reclaimable(&self, admission: &crate::admission::Admission) -> u64 {
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
    /// for that engine, has not been unloaded, and its manifests/language server remain fresh.
    pub(crate) fn reusable_for(&self, engine: &str) -> bool {
        if self.engine != engine || self.has_dead_server() {
            return false;
        }
        if self.unloaded.load(Ordering::SeqCst) {
            return false;
        }
        if self.has_stale_manifests() {
            return false;
        }
        true
    }

    /// Whether this workspace's dependency manifests or lockfiles have changed on disk.
    pub fn has_stale_manifests(&self) -> bool {
        if self.unloaded.load(Ordering::SeqCst) {
            return true;
        }
        if let Some(ref base) = self.base_workspace
            && base.has_stale_manifests()
        {
            return true;
        }
        if let Some(ref eng_arc) = self.rust_engine
            && let Ok(eng) = eng_arc.try_lock()
        {
            if self.base_workspace.is_some() {
                if eng.is_worktree_stale(&self.root) {
                    return true;
                }
            } else if eng.is_base_stale() {
                return true;
            }
        }
        for (path, recorded_mtime) in &self.manifest_mtimes {
            let current = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if current != *recorded_mtime {
                return true;
            }
        }
        false
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
            if val_attached && let Some(Some(val_eng)) = base.validation.get() {
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
