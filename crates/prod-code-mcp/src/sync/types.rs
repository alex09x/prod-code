/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::{FileDelta, FileStamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10 MiB per source file limit
pub const MAX_LIBRARY_SIZE: u64 = 128 * 1024 * 1024;
pub const SYNC_BATCH_BYTES: usize = 24 * 1024 * 1024;
pub const MAX_JSON_CONFIG_SIZE: u64 = 8 * 1024 * 1024; // 8 MiB for .json configs/metadata
pub const MAX_NON_GIT_WORKSPACE_FILES: usize = 10_000;
pub const MAX_NON_GIT_WORKSPACE_BYTES: usize = 256 * 1024 * 1024; // 256 MiB total
pub const RELEVANCE_VERSION: u32 = 13;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncFileEntry {
    pub mtime_sec: u64,
    pub mtime_nsec: u32,
    pub size: u64,
    #[serde(default)]
    pub hash: u64,
    /// Executable bit of the copy the node holds (#1823). `None` is a watermark written before
    /// modes were tracked: compared as "not executable", so only executable files are sent
    /// again, and kept out of the probe manifest, so the gateway does not judge an unknown mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<bool>,
}

impl SyncFileEntry {
    /// Whether the node already holds the file this entry describes: same stamp, same mode.
    pub(crate) fn matches(&self, current: &SyncFileEntry) -> bool {
        self.mtime_sec == current.mtime_sec
            && self.mtime_nsec == current.mtime_nsec
            && self.size == current.size
            && self.hash == current.hash
            && self.executable.unwrap_or(false) == current.executable.unwrap_or(false)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncCache {
    #[serde(default)]
    pub base_commit_sha: Option<String>,
    #[serde(default)]
    pub last_sync_timestamp_ms: u64,
    #[serde(default)]
    pub files: HashMap<String, SyncFileEntry>,
    /// Paths that were dirty or untracked at the last full sync. A path that later drops out of
    /// this set without a commit was reverted and must be sent again in its clean form.
    #[serde(default)]
    pub dirty_paths: BTreeSet<String>,
    /// [`RELEVANCE_VERSION`] the watermark was built with. When the relevance filter learns
    /// about new file kinds, older watermarks would wrongly assume those files were sent.
    #[serde(default)]
    pub filter_version: u32,
    /// Paths the gateway removed from its copy because a command changed them after its client
    /// left and it could not put them back (#262). Git sees no change in them, so they are sent
    /// by name on the next sync.
    #[serde(default)]
    pub resend: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceIdentity {
    pub name: String,
    pub base: Option<String>,
}

#[derive(Debug)]
pub struct SyncPlan {
    pub files: Vec<FileDelta>,
    pub omitted: Vec<crate::sync::UploadExclusion>,
    pub(crate) state: SyncCache,
    pub(crate) node: String,
    pub initial: bool,
}

impl SyncPlan {
    pub fn manifest(&self) -> Vec<FileStamp> {
        let mut stamps: Vec<FileStamp> = self
            .state
            .files
            .iter()
            .map(|(path, entry)| FileStamp {
                relative_path: path.clone(),
                size: entry.size,
                hash: entry.hash,
                is_executable: entry.executable,
            })
            .collect();
        stamps.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        stamps
    }

    pub fn retain_uploads(&mut self, keep: &HashSet<String>) {
        self.files
            .retain(|f| f.content.is_none() || keep.contains(&f.relative_path));
    }
}

#[derive(Debug, Clone, Default)]
pub struct SyncOutcome {
    pub planned: usize,
    pub files_updated: usize,
    pub files_deleted: usize,
    pub bytes_transferred: usize,
    pub probed: bool,
    pub seeded: bool,
    pub server_workspace_root: String,
    pub changed_paths: Vec<String>,
    pub stale_paths: Vec<String>,
    pub omitted: Vec<crate::sync::UploadExclusion>,
    pub node: String,
}

#[derive(Debug)]
pub(crate) enum RoundResult {
    Success(SyncOutcome),
    NeedsFullResync,
}

#[derive(Default)]
pub(crate) struct SyncStageTracker {
    stage: std::sync::Mutex<Option<&'static str>>,
    parent: Option<std::sync::Arc<SyncStageTracker>>,
}

tokio::task_local! {
    static ACTIVE_SYNC_STAGE: std::sync::Arc<SyncStageTracker>;
}

impl SyncStageTracker {
    pub(crate) fn new() -> std::sync::Arc<Self> {
        let parent = ACTIVE_SYNC_STAGE.try_with(std::sync::Arc::clone).ok();
        std::sync::Arc::new(Self {
            stage: std::sync::Mutex::new(None),
            parent,
        })
    }

    pub(crate) fn get(&self) -> Option<&'static str> {
        *self.stage.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, next: Option<&'static str>) {
        *self.stage.lock().unwrap_or_else(|e| e.into_inner()) = next;
        if let Some(parent) = &self.parent {
            parent.set(next);
        }
    }
}

pub(crate) async fn scope_sync_stage<F: std::future::Future>(
    tracker: std::sync::Arc<SyncStageTracker>,
    fut: F,
) -> F::Output {
    ACTIVE_SYNC_STAGE.scope(tracker, fut).await
}

pub(crate) fn set_active_sync_stage(stage: Option<&'static str>) {
    let _ = ACTIVE_SYNC_STAGE.try_with(|tracker| tracker.set(stage));
}

/// Stage of a call that waits for the checkout's pooled session, owned by another query (#1814).
pub(crate) const QUEUED_FOR_SESSION: &str = "waiting for another query in this workspace";
/// Stage of a call that waits for a workspace write (an apply, a command) to release the checkout.
pub(crate) const QUEUED_FOR_WRITER: &str = "waiting for a workspace write in this checkout";

/// Whether `stage` is a wait in this client's queue: the analyzer was not asked yet.
pub(crate) fn is_queue_stage(stage: &str) -> bool {
    stage == QUEUED_FOR_SESSION || stage == QUEUED_FOR_WRITER
}
