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
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(test)]
mod tests;

/// The file, in the gateway's storage.
const FILE: &str = "recent-workspaces.json";
/// More than this are not kept: the oldest go.
const MAX_KEPT: usize = 64;
/// A workspace used again within this long is not written again: a request is not a reason to
/// touch the disk.
const DEBOUNCE_MS: u64 = 60_000;

/// A workspace the gateway loaded, and when it was last used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recent {
    pub root: PathBuf,
    pub engine: String,
    pub last_used_ms: u64,
}

/// The workspaces in use lately, newest first, kept in a file so that a restart can load them
/// again.
pub struct RecentWorkspaces {
    path: PathBuf,
    entries: Mutex<Vec<Recent>>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl RecentWorkspaces {
    /// The record in `storage`, as the last run left it; a missing or unreadable file is an
    /// empty record, never an error.
    pub fn open(storage: &Path) -> Self {
        let path = storage.join(FILE);
        let entries = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Vec<Recent>>(&text).unwrap_or_else(|error| {
                tracing::warn!(path = %path.display(), %error, "ignoring an unreadable record of recent workspaces");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Self {
            path,
            entries: Mutex::new(entries),
        }
    }

    /// Notes that `engine` was used on `root` now.
    pub fn touch(&self, root: &Path, engine: &str) {
        self.touch_at(root, engine, now_ms());
    }

    pub(crate) fn touch_at(&self, root: &Path, engine: &str, now_ms: u64) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(known) = entries
            .iter()
            .find(|e| e.root == root && e.engine == engine)
            && now_ms.saturating_sub(known.last_used_ms) < DEBOUNCE_MS
        {
            return;
        }
        entries.retain(|e| !(e.root == root && e.engine == engine));
        entries.insert(
            0,
            Recent {
                root: root.to_path_buf(),
                engine: engine.to_string(),
                last_used_ms: now_ms,
            },
        );
        entries.truncate(MAX_KEPT);
        self.save(&entries);
    }

    /// At most `count` workspaces used within `max_age` of `now_ms`, the newest first.
    pub fn newest(&self, count: usize, max_age: Duration, now_ms: u64) -> Vec<Recent> {
        let oldest = now_ms.saturating_sub(max_age.as_millis() as u64);
        let mut found: Vec<Recent> = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|e| e.last_used_ms >= oldest)
            .cloned()
            .collect();
        found.sort_by_key(|entry| std::cmp::Reverse(entry.last_used_ms));
        found.truncate(count);
        found
    }

    /// Writes the record through a temporary file, so that a crash leaves the old one whole.
    fn save(&self, entries: &[Recent]) {
        let tmp = self.path.with_extension("json.tmp");
        let written = serde_json::to_vec(entries)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&tmp, bytes))
            .and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(error) = written {
            tracing::warn!(path = %self.path.display(), %error, "could not write the record of recent workspaces");
        }
    }
}
