/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Workspace loading, manifest discovery, and Salsa database initialization.

use anyhow::Result;
use ra_ap_ide::AnalysisHost;
use ra_ap_ide_db::FxHashMap;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, worktrees::Worktrees};
use ra_ap_paths::AbsPathBuf;
use ra_ap_project_model::{ProjectManifest, ProjectWorkspace};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::RustEngine;
use crate::config::{ProcMacroServerKind, ProdCodeConfig};
use crate::load_budget;
use crate::proc_macro_farm::{self, FarmMetrics};
use crate::session_overlays::SessionOverlays;

impl RustEngine {
    /// Detect whether a directory contains a Rust workspace manifest (Cargo.toml).
    pub fn is_rust_workspace(path: &Path) -> bool {
        path.join("Cargo.toml").exists()
    }

    /// Load and index a Cargo workspace directly into in-memory Salsa DB using multi-core worker threads.
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let config = ProdCodeConfig::load(workspace_root);
        tracing::info!(?workspace_root, rust = ?config.rust, "analysis options");
        let budget = load_budget::shared();
        let waiting = std::time::Instant::now();
        let _permit = budget.acquire();
        let num_threads = budget.workers;
        let mut cargo_config = config.cargo_config();
        // Apply to this invocation only; changing the daemon's environment races other
        // loads and user commands. The CLI argument also bounds a project's jobs setting.
        cargo_config
            .extra_args
            .push(format!("--jobs={num_threads}"));
        let build_scripts = config.rust.build_scripts;
        let farm = proc_macro_farm::shared();
        let desired_workers = config.rust.proc_macro_workers.unwrap_or(1);

        tracing::info!(
            ?workspace_root,
            threads = num_threads,
            queue_ms = waiting.elapsed().as_millis() as u64,
            "Loading Cargo workspace with a bounded CPU budget"
        );
        let abs_root = if workspace_root.is_absolute() {
            AbsPathBuf::assert_utf8(workspace_root.to_path_buf())
        } else {
            AbsPathBuf::assert_utf8(std::env::current_dir()?.join(workspace_root))
        };
        let manifest = ProjectManifest::discover_single(&abs_root)
            .map_err(|e| anyhow::anyhow!("Manifest discovery failed: {e}"))?;
        let mut ws = ProjectWorkspace::load(manifest, &cargo_config, &|_| {})
            .map_err(|e| anyhow::anyhow!("Project workspace load failed: {e}"))?;
        if build_scripts && let Ok(scripts) = ws.run_build_scripts(&cargo_config, &|_| {}) {
            ws.set_build_scripts(scripts);
        }

        let (proc_macro_choice, proc_macro_processes, farm_permit) = match config
            .rust
            .proc_macro_srv
        {
            ProcMacroServerKind::Disabled => (ProcMacroServerChoice::None, 0, None),
            _ if !build_scripts => (ProcMacroServerChoice::None, 0, None),
            ProcMacroServerKind::Sysroot => {
                let (workers, permit) = farm.allocate_workers_timeout(
                    workspace_root,
                    desired_workers,
                    std::time::Duration::from_millis(2000),
                );
                if workers == 0 {
                    tracing::warn!(
                        workspace = %workspace_root.display(),
                        capacity = farm.capacity(),
                        "Proc-macro farm is at capacity; proc-macro server disabled for workspace"
                    );
                    (ProcMacroServerChoice::None, 0, None)
                } else {
                    (ProcMacroServerChoice::Sysroot, workers, Some(permit))
                }
            }
            ProcMacroServerKind::Sandboxed => {
                let (workers, permit) = farm.allocate_workers_timeout(
                    workspace_root,
                    desired_workers,
                    std::time::Duration::from_millis(2000),
                );
                if workers == 0 {
                    tracing::warn!(
                        workspace = %workspace_root.display(),
                        capacity = farm.capacity(),
                        "Proc-macro farm is at capacity; proc-macro server disabled for workspace"
                    );
                    (ProcMacroServerChoice::None, 0, None)
                } else {
                    let memory_limit_mb = config.rust.proc_macro_memory_limit_mb.unwrap_or(2048);
                    let sysroot_srv = ws
                        .find_sysroot_proc_macro_srv()
                        .and_then(|res| res.ok())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Sandboxed proc-macro server required but sysroot rust-analyzer-proc-macro-srv not found. \
                                Configure [rust] proc_macro_srv = \"sysroot\" to explicitly opt out of sandboxing."
                            )
                        })?;
                    let wrapper = proc_macro_farm::prepare_sandboxed_srv(
                        sysroot_srv.as_ref(),
                        memory_limit_mb,
                    )
                    .map_err(|err| {
                        anyhow::anyhow!(
                            "Failed to prepare sandboxed proc-macro server: {err}. \
                            Refusing to fall back to unsandboxed execution in sandboxed mode."
                        )
                    })?;
                    (
                        ProcMacroServerChoice::Explicit(AbsPathBuf::assert_utf8(wrapper)),
                        workers,
                        Some(permit),
                    )
                }
            }
        };

        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: build_scripts,
            with_proc_macro_server: proc_macro_choice,
            prefill_caches: false,
            num_worker_threads: num_threads,
            proc_macro_processes,
        };

        let (worktrees, db, vfs) = Worktrees::load(ws, &FxHashMap::default(), &load_config)
            .map_err(|e| anyhow::anyhow!("Failed to load cargo workspace: {e}"))?;

        let host = AnalysisHost::with_database(db);
        tracing::info!(?workspace_root, "Cargo workspace warm and ready in RAM");

        Ok(Self {
            workspace_root: workspace_root.to_path_buf(),
            host,
            vfs: Arc::new(std::sync::RwLock::new(vfs)),
            worktrees,
            worktree_attachments: HashMap::new(),
            overlays: SessionOverlays::default(),
            label: "main",
            changes: 0,
            _proc_macro_farm_permit: farm_permit,
        })
    }

    /// Returns current metrics for the node-wide shared proc-macro worker farm.
    pub fn proc_macro_farm_metrics() -> FarmMetrics {
        proc_macro_farm::shared().metrics()
    }
}
