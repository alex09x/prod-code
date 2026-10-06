/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Workspace management: multi-tenant shared workspaces, leader-follower coalescing, and worktree views.

pub(crate) mod loader;
pub(crate) mod manager;
pub(crate) mod paths;
pub(crate) mod probes;
pub(crate) mod prune;
pub(crate) mod session;
pub(crate) mod shared;
pub(crate) mod stale;
pub(crate) mod types;
pub(crate) mod validation;

#[cfg(test)]
mod tests;

pub use manager::{RECLAIM_MIN_IDLE, WorkspaceManager};
pub use paths::{
    resolve_server_workspace, sanitize_identifier, server_workspace_path, split_worktree_base,
    worktree_suffix,
};
pub use prune::{
    LAST_USED_MARKER, free_and_total_bytes, free_share, prune_stale_main_workspace_dirs,
    prune_stale_worktree_dirs, prune_worktree_dirs_for_space, touch_last_used, touch_last_used_at,
};
pub use session::{
    DirectEditLeaseHandle, SessionView, WorkspaceLease, WorktreeEntry, WorktreeOwner,
    migrate_direct_edits_to_overlays,
};
pub use shared::{SharedWorkspace, watched_events};
pub use stale::{
    STALE_MARKER, clear_stale_paths, forget_stale_paths, record_stale_paths, record_synced,
    stale_paths, synced_since,
};
pub use types::{
    RustEngines, RustLoader, WatchedChange, WorkspaceKey, default_max_concurrent_engine_loads,
    unix_now,
};
