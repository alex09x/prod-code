/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// Metrics snapshot of the shared proc-macro worker farm.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FarmMetrics {
    /// Maximum worker processes allowed across the entire gateway node.
    pub capacity: usize,
    /// Currently allocated worker processes across all active workspaces.
    pub active_workers: usize,
    /// Number of active workspaces holding worker permits.
    pub active_workspaces: usize,
    /// Default virtual address space limit in megabytes for sandboxed workers.
    pub default_memory_limit_mb: u64,
}

/// Node-wide shared proc-macro worker farm managing worker process concurrency.
#[derive(Debug)]
pub struct ProcMacroWorkerFarm {
    /// Global worker process capacity.
    capacity: usize,
    /// Total worker processes currently allocated.
    active_workers: Mutex<usize>,
    /// Active workspace allocations: workspace root -> allocated worker count.
    allocations: Mutex<HashMap<PathBuf, usize>>,
    /// Condition variable to notify waiting workspace allocations when permits are released.
    cvar: Condvar,
}

impl ProcMacroWorkerFarm {
    /// Creates a new worker farm with a specified global worker capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            active_workers: Mutex::new(0),
            allocations: Mutex::new(HashMap::new()),
            cvar: Condvar::new(),
        }
    }

    /// Global worker capacity across all workspaces.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Total worker processes currently allocated across all active workspaces.
    pub fn active_workers(&self) -> usize {
        *self
            .active_workers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Number of active workspaces holding worker permits.
    pub fn active_workspaces(&self) -> usize {
        self.allocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Returns a telemetry snapshot of the worker farm.
    pub fn metrics(&self) -> FarmMetrics {
        FarmMetrics {
            capacity: self.capacity(),
            active_workers: self.active_workers(),
            active_workspaces: self.active_workspaces(),
            default_memory_limit_mb: 2048,
        }
    }

    /// Allocates worker processes for a workspace from the shared farm immediately.
    ///
    /// The allocated count is bounded by the global farm capacity and fair-share limits.
    pub fn allocate_workers(
        self: &Arc<Self>,
        workspace_root: &Path,
        desired: usize,
    ) -> (usize, ProcMacroFarmPermit) {
        self.allocate_workers_timeout(workspace_root, desired, Duration::ZERO)
    }

    /// Allocates worker processes for a workspace with fair-share capacity governance
    /// and optional timeout queuing.
    pub fn allocate_workers_timeout(
        self: &Arc<Self>,
        workspace_root: &Path,
        desired: usize,
        timeout: Duration,
    ) -> (usize, ProcMacroFarmPermit) {
        if desired == 0 {
            return (
                0,
                ProcMacroFarmPermit {
                    farm: Arc::clone(self),
                    workspace: workspace_root.to_path_buf(),
                    count: 0,
                },
            );
        }

        // Fair share ceiling: ensure multi-tenant fairness so a single workspace
        // cannot monopolize the farm and starve subsequent workspaces.
        // On nodes with capacity <= 4, allow at most 1 worker per workspace.
        // On larger nodes, allow at most capacity / 4 workers (clamped to 1..4).
        let fair_share = if self.capacity <= 4 {
            1
        } else {
            (self.capacity / 4).clamp(1, 4)
        };
        let target = desired.min(fair_share);

        let mut active = self
            .active_workers
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        if *active >= self.capacity && !timeout.is_zero() {
            match self
                .cvar
                .wait_timeout_while(active, timeout, |act| *act >= self.capacity)
            {
                Ok((new_active, _)) => {
                    active = new_active;
                }
                Err(e) => {
                    active = e.into_inner().0;
                }
            }
        }

        let remaining = self.capacity.saturating_sub(*active);
        let allocated = target.min(remaining);

        if allocated == 0 {
            tracing::warn!(
                workspace = %workspace_root.display(),
                active_workers = *active,
                farm_capacity = self.capacity,
                "Proc-macro farm is at capacity across all active workspaces; worker allocation rejected"
            );
            return (
                0,
                ProcMacroFarmPermit {
                    farm: Arc::clone(self),
                    workspace: workspace_root.to_path_buf(),
                    count: 0,
                },
            );
        }

        let mut allocs = self.allocations.lock().unwrap_or_else(|e| e.into_inner());
        *active += allocated;
        *allocs.entry(workspace_root.to_path_buf()).or_insert(0) += allocated;

        tracing::info!(
            workspace = %workspace_root.display(),
            allocated_workers = allocated,
            total_active_workers = *active,
            farm_capacity = self.capacity,
            "Allocated proc-macro farm workers"
        );

        (
            allocated,
            ProcMacroFarmPermit {
                farm: Arc::clone(self),
                workspace: workspace_root.to_path_buf(),
                count: allocated,
            },
        )
    }

    pub(crate) fn release(&self, workspace: &Path, count: usize) {
        if count == 0 {
            return;
        }
        let mut active = self
            .active_workers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut allocs = self.allocations.lock().unwrap_or_else(|e| e.into_inner());

        let freed = if let Some(current) = allocs.get_mut(workspace) {
            let to_remove = (*current).min(count);
            *current -= to_remove;
            if *current == 0 {
                allocs.remove(workspace);
            }
            to_remove
        } else {
            0
        };

        *active = active.saturating_sub(freed);
        drop(allocs);
        drop(active);
        self.cvar.notify_all();
    }

    /// Access the shared node-level process-global farm singleton.
    pub fn shared() -> &'static Arc<ProcMacroWorkerFarm> {
        static FARM: OnceLock<Arc<ProcMacroWorkerFarm>> = OnceLock::new();
        FARM.get_or_init(default_node_farm)
    }
}

/// Access the shared node-level process-global farm singleton.
pub fn shared() -> &'static Arc<ProcMacroWorkerFarm> {
    ProcMacroWorkerFarm::shared()
}

/// RAII permit for worker processes allocated to an active workspace.
///
/// When the `RustEngine` or workspace is dropped or evicted from LRU cache,
/// the permit's `Drop` implementation automatically returns the allocated
/// worker process quota back to the shared `ProcMacroWorkerFarm`.
#[derive(Debug)]
pub struct ProcMacroFarmPermit {
    farm: Arc<ProcMacroWorkerFarm>,
    workspace: PathBuf,
    count: usize,
}

impl ProcMacroFarmPermit {
    /// Number of worker processes held by this permit.
    pub fn worker_count(&self) -> usize {
        self.count
    }

    /// Associated workspace root path.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
}

impl Drop for ProcMacroFarmPermit {
    fn drop(&mut self) {
        self.farm.release(&self.workspace, self.count);
    }
}

/// Computes the default node-level farm capacity based on available CPU cores.
fn default_node_farm() -> Arc<ProcMacroWorkerFarm> {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    let default_capacity = if let Ok(val) = std::env::var("PROD_CODE_PROC_MACRO_WORKERS") {
        val.parse::<usize>()
            .unwrap_or_else(|_| (cpus / 2).clamp(2, 16))
    } else {
        (cpus / 2).clamp(2, 16)
    };

    tracing::info!(
        capacity = default_capacity,
        cpus = cpus,
        "Initialized shared proc-macro worker farm"
    );

    Arc::new(ProcMacroWorkerFarm::new(default_capacity))
}
