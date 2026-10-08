/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod cache;
pub mod engine;
pub mod entry;
pub mod filter_path;
pub mod git;
pub mod plan;
pub mod pull;
pub mod push;
pub(crate) mod read;
pub mod relevance;
pub mod scan;
pub mod types;

#[cfg(test)]
mod tests;

pub use cache::{
    clear_sync_cache, clear_sync_cache_for, forget_synced_files, load_sync_cache,
    load_sync_cache_for, resend_lost_files, save_sync_cache, save_sync_cache_for,
    workspace_identity,
};
pub use engine::{
    engine_for_file, engine_project, expected_engine, is_in_dependency_dir, macos_only_cgo,
    other_checkout,
};
pub use filter_path::{is_filesystem_root, is_fixture_path, is_synced_git_path};
pub use plan::{commit_workspace_sync, prepare_workspace_sync, prepare_workspace_sync_for};
pub use pull::{apply_pulled_files, apply_pulled_files_for, pull_remote_files};
pub use push::{gateway_node, push_workspace_sync};
pub use relevance::is_relevant_code_or_manifest_file;
pub use scan::{collect_dirty_files, collect_dirty_files_incremental, scan_workspace_files};
pub use types::{
    MAX_FILE_SIZE, MAX_JSON_CONFIG_SIZE, MAX_LIBRARY_SIZE, MAX_NON_GIT_WORKSPACE_BYTES,
    MAX_NON_GIT_WORKSPACE_FILES, RELEVANCE_VERSION, SYNC_BATCH_BYTES, SyncCache, SyncFileEntry,
    SyncOutcome, SyncPlan, WorkspaceIdentity,
};
