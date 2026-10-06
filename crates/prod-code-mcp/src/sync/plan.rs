/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::cache::{load_sync_cache_for, save_sync_cache_for};
use crate::sync::entry::{fits_sync, is_executable, sync_file_entry};
use crate::sync::filter_path::{SyncPathFilter, is_synced_git_path};
use crate::sync::git::{changed_paths, git_head, git_listed_paths};
use crate::sync::scan::{read_regular_file_secure, scan_workspace_files};
use crate::sync::types::{RELEVANCE_VERSION, SyncCache, SyncPlan};
use anyhow::{Context, Result};
use prod_code_protocol::FileDelta;
use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Build a sync plan from the last acknowledged git base plus the current working tree.
///
/// The first plan for a worktree includes tracked source/manifest files. Later plans use both
/// `git diff <base>` and `git status --porcelain -uall`, then verify candidates against the
/// persisted mtime/size/hash watermark before reading them.
pub fn prepare_workspace_sync(root: &Path, subpath: Option<&Path>) -> Result<SyncPlan> {
    prepare_workspace_sync_for(root, "", subpath)
}

/// [`prepare_workspace_sync`] against the watermark of gateway `node`.
pub fn prepare_workspace_sync_for(
    root: &Path,
    node: &str,
    subpath: Option<&Path>,
) -> Result<SyncPlan> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut state = load_sync_cache_for(&canonical_root, node);
    if state.filter_version != RELEVANCE_VERSION && subpath.is_none() {
        state.base_commit_sha = None;
        state.files.clear();
        state.filter_version = RELEVANCE_VERSION;
    }
    let initial = state.base_commit_sha.is_none() && subpath.is_none();
    let current_base = match git_head(&canonical_root) {
        Ok(base) => base,
        Err(_) => {
            return prepare_non_git_workspace_sync(&canonical_root, node, subpath, state);
        }
    };
    let (mut changes, current_dirty) = changed_paths(
        &canonical_root,
        state.base_commit_sha.as_deref(),
        &current_base,
    )?;
    // A file that was dirty last time and is clean now without a commit was reverted: the
    // gateway still holds the dirty version, so send the clean one (or its deletion).
    for reverted in state.dirty_paths.difference(&current_dirty) {
        if !changes.contains_key(reverted) {
            let exists = canonical_root.join(reverted).is_file();
            changes.insert(reverted.clone(), !exists);
        }
    }
    let filter = SyncPathFilter::new(&canonical_root, subpath)?;
    let mut files = Vec::new();

    for (relative_path, deleted) in changes {
        // Every path here came from git: tracked, or untracked and not ignored.
        if !filter.includes(&relative_path) || !is_synced_git_path(&relative_path) {
            continue;
        }

        if deleted {
            files.push(FileDelta {
                relative_path: relative_path.clone(),
                content: None,
                is_executable: false,
            });
            state.files.remove(&relative_path);
            continue;
        }

        let full_path = canonical_root.join(&relative_path);
        let Ok(sym_meta) = full_path.symlink_metadata() else {
            continue;
        };
        if sym_meta.file_type().is_symlink() {
            continue;
        }
        if !fits_sync(&relative_path, &sym_meta) {
            continue;
        }

        let Some((content, is_exec)) = read_regular_file_secure(&full_path, &canonical_root)?
        else {
            continue;
        };
        let entry = sync_file_entry(&sym_meta, &content);
        if state.files.get(&relative_path) == Some(&entry) {
            continue;
        }

        files.push(FileDelta {
            relative_path: relative_path.clone(),
            content: Some(content),
            is_executable: is_exec,
        });
        state.files.insert(relative_path, entry);
    }

    // Files the gateway lost (#262) are sent whatever git says about them: their content when
    // a sync carries such a file, their deletion otherwise, so that the gateway's record of them
    // is cleared either way. One outside a partial sync waits for a sync that covers it.
    let mut lost = Vec::new();
    for rel in std::mem::take(&mut state.resend) {
        if !filter.includes(&rel) {
            state.resend.insert(rel);
        } else if !files.iter().any(|f| f.relative_path == rel) {
            lost.push(rel);
        }
    }
    let listed = git_listed_paths(&canonical_root, &lost);
    for rel in lost {
        let full_path = canonical_root.join(&rel);
        match full_path.symlink_metadata() {
            Ok(sym_meta) => {
                if sym_meta.file_type().is_symlink()
                    || !listed.contains(&rel)
                    || !is_synced_git_path(&rel)
                    || !fits_sync(&rel, &sym_meta)
                {
                    state.files.remove(&rel);
                    files.push(FileDelta {
                        relative_path: rel,
                        content: None,
                        is_executable: false,
                    });
                    continue;
                }
                let Some((content, is_executable)) =
                    read_regular_file_secure(&full_path, &canonical_root)?
                else {
                    state.resend.insert(rel);
                    continue;
                };
                state
                    .files
                    .insert(rel.clone(), sync_file_entry(&sym_meta, &content));
                files.push(FileDelta {
                    relative_path: rel,
                    content: Some(content),
                    is_executable,
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                state.files.remove(&rel);
                files.push(FileDelta {
                    relative_path: rel,
                    content: None,
                    is_executable: false,
                });
            }
            Err(err) => return Err(err.into()),
        }
    }

    // A partial sync cannot advance the workspace-wide base: changes outside the selected path
    // still need to be included by the next full sync.
    if subpath.is_none() {
        state.base_commit_sha = Some(current_base);
        state.dirty_paths = current_dirty;
    }
    Ok(SyncPlan {
        files,
        state,
        node: node.to_string(),
        initial,
    })
}

/// A manifest-based checkout has no Git baseline for a diff. Walk its relevant source tree and
/// compare file stamps with the acknowledged watermark instead, retaining deletions from the
/// previous scan and honoring explicit resend requests.
fn prepare_non_git_workspace_sync(
    root: &Path,
    node: &str,
    subpath: Option<&Path>,
    mut state: SyncCache,
) -> Result<SyncPlan> {
    if state.filter_version != RELEVANCE_VERSION {
        state.files.clear();
        state.filter_version = RELEVANCE_VERSION;
    }
    let filter = SyncPathFilter::new(root, subpath)?;
    let previous: HashSet<String> = state
        .files
        .keys()
        .filter(|path| filter.includes(path))
        .cloned()
        .collect();
    let mut resend = HashSet::new();
    for path in std::mem::take(&mut state.resend) {
        if filter.includes(&path) {
            state.files.remove(&path);
            resend.insert(path);
        } else {
            state.resend.insert(path);
        }
    }

    let scanned = scan_workspace_files(root, subpath)?;
    let mut files = Vec::new();
    let mut present = HashSet::new();
    for mut delta in scanned {
        let Some(content) = delta.content.as_deref() else {
            continue;
        };
        let relative_path = delta.relative_path.clone();
        let full_path = root.join(&relative_path);
        let metadata = std::fs::symlink_metadata(&full_path)
            .with_context(|| format!("cannot stat synced file {}", full_path.display()))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let entry = sync_file_entry(&metadata, content);
        present.insert(delta.relative_path.clone());
        if state.files.get(&relative_path) != Some(&entry) {
            delta.is_executable = is_executable(&metadata);
            files.push(delta);
        }
        state.files.insert(relative_path, entry);
    }

    for path in previous.union(&resend) {
        if !present.contains(path) && !files.iter().any(|delta| delta.relative_path == *path) {
            files.push(FileDelta {
                relative_path: path.clone(),
                content: None,
                is_executable: false,
            });
            state.files.remove(path);
        }
    }

    // No Git commit represents this workspace. Keeping the base absent makes a later sync
    // continue to use the manifest probe instead of failing on `git rev-parse HEAD`.
    state.base_commit_sha = None;
    state.dirty_paths.clear();
    Ok(SyncPlan {
        files,
        state,
        node: node.to_string(),
        initial: subpath.is_none(),
    })
}

/// Persist the watermarks for a sync plan after its files have been accepted by the gateway.
pub fn commit_workspace_sync(root: &Path, plan: &SyncPlan) {
    let mut state = plan.state.clone();
    state.last_sync_timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    save_sync_cache_for(root, &plan.node, &state);
}
