/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use tokio::sync::Mutex;

use super::probes::wait_for_swift_build_settings;
use super::shared::SharedWorkspace;
use super::types::{RustLoader, unix_now};

impl SharedWorkspace {
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
                manifest_mtimes: self.manifest_mtimes.clone(),
            }));
        }
        self.base_validation_view(admission).await
    }

    pub(crate) async fn base_validation_view(
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
            manifest_mtimes: self.manifest_mtimes.clone(),
        }))
    }

    /// The validation engine, loaded with `load` by the first session that asks while the host
    /// has memory for it. The load runs in a task of its own: a session that stops waiting
    /// leaves it running with its reservation, and the next session waits for its engine
    /// instead of loading another beside it.
    pub(crate) async fn validation_engine(
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
    pub(crate) async fn generic_validation_view(
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
            manifest_mtimes: self.manifest_mtimes.clone(),
        }))
    }
}
