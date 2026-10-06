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
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use super::super::loader::LoadState;
use super::super::session::WorktreeOwners;
use super::super::types::{RustLoader, WorkspaceKey, default_max_concurrent_engine_loads};

/// How long an engine must have been without a session before a load that finds no memory may
/// unload it. An agent's commands come in bursts with short gaps between them, and an engine
/// unloaded in such a gap is loaded again, cold, by the next command.
pub const RECLAIM_MIN_IDLE: Duration = Duration::from_secs(120);

/// Thread-safe manager coordinating workspace lifecycle and leader-follower loading.
pub struct WorkspaceManager {
    pub(crate) workspaces: RwLock<HashMap<WorkspaceKey, LoadState>>,
    pub(crate) worktree_owners: WorktreeOwners,
    /// The language servers of editors' sessions, which run outside the shared workspaces.
    pub editor_servers: crate::editor_proxy::EditorServers,
    /// Whether the host has memory for another engine (#433).
    pub(crate) admission: Arc<crate::admission::Admission>,
    pub(crate) rust_loader: RustLoader,
    pub(crate) load_semaphore: Arc<tokio::sync::Semaphore>,
}

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
    pub(crate) fn with_rust_loader(mut self, rust_loader: RustLoader) -> Self {
        self.rust_loader = rust_loader;
        self
    }

    pub fn admission(&self) -> &Arc<crate::admission::Admission> {
        &self.admission
    }
}
