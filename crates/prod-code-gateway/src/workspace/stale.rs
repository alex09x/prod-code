/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

/// The file in a workspace's copy on the node listing checkout-relative paths that are missing from
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
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default()
}

fn write_stale_paths(workspace_dir: &Path, paths: &BTreeSet<String>) {
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
    let mut all: BTreeSet<String> = stale_paths(workspace_dir).into_iter().collect();
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
type SyncedFiles = HashMap<String, (Instant, Option<u64>)>;

static SYNCED: LazyLock<Mutex<HashMap<PathBuf, SyncedFiles>>> = LazyLock::new(Default::default);

/// Notes that a sync just wrote (or deleted) `files` in the workspace copy.
pub fn record_synced(workspace_dir: &Path, files: &[(String, Option<u64>)]) {
    if files.is_empty() {
        return;
    }
    let now = Instant::now();
    let mut all = SYNCED.lock().unwrap_or_else(|e| e.into_inner());
    let known = all.entry(workspace_dir.to_path_buf()).or_default();
    for (path, hash) in files {
        known.insert(path.clone(), (now, *hash));
    }
}

/// The files a sync delivered to the workspace copy at or after `since`, with the hash of the
/// text it wrote (`None` for a deletion).
pub fn synced_since(workspace_dir: &Path, since: Instant) -> HashMap<String, Option<u64>> {
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
    let mut all: BTreeSet<String> = stale_paths(workspace_dir).into_iter().collect();
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
