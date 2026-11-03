/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::entry::fits_sync;
use crate::sync::filter_path::{is_filesystem_root, is_synced_git_path, is_synced_git_path_with};
use crate::sync::git::{collect_git_dirty_files, git_listed_files};
use crate::sync::include::{SyncInclude, is_non_overridable_sync_path};
use crate::sync::participation::{UploadExclusion, UploadExclusionReason, is_non_git_ignored};
use crate::sync::read::read_file_or_contained_symlink;
use crate::sync::relevance::is_relevant_code_or_manifest_file;
use crate::sync::types::{
    MAX_FILE_SIZE, MAX_JSON_CONFIG_SIZE, MAX_NON_GIT_WORKSPACE_BYTES, MAX_NON_GIT_WORKSPACE_FILES,
};
use anyhow::Result;
use prod_code_protocol::FileDelta;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;

/// Scan workspace directory and generate FileDelta list, filtering out build artifacts and VCS.
///
/// In a git checkout the list is git's (see [`is_synced_git_path_with`]); elsewhere the directory
/// is walked and filtered by [`is_relevant_code_or_manifest_file`] plus `[sync] include`.
pub fn scan_workspace_files(root: &Path, subpath: Option<&Path>) -> Result<Vec<FileDelta>> {
    scan_workspace_files_with_omitted(root, subpath).map(|(deltas, _)| deltas)
}

pub(crate) fn scan_workspace_files_with_omitted(
    root: &Path,
    subpath: Option<&Path>,
) -> Result<(Vec<FileDelta>, Vec<UploadExclusion>)> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if is_filesystem_root(&canonical_root) {
        anyhow::bail!(
            "refusing to scan filesystem root: {}",
            canonical_root.display()
        );
    }
    let target_dir = match subpath {
        Some(sub) => {
            if sub.is_absolute() {
                std::fs::canonicalize(sub).unwrap_or_else(|_| sub.to_path_buf())
            } else {
                let joined = canonical_root.join(sub);
                std::fs::canonicalize(&joined).unwrap_or(joined)
            }
        }
        None => canonical_root.clone(),
    };

    if !target_dir.exists() {
        anyhow::bail!("Path {:?} does not exist", target_dir);
    }

    let sync_include = SyncInclude::load(&canonical_root);
    let mut deltas = Vec::new();
    let mut omitted = Vec::new();

    if target_dir.is_file() {
        let rel_path = target_dir
            .strip_prefix(&canonical_root)
            .unwrap_or(&target_dir)
            .to_string_lossy()
            .to_string();
        let rel = Path::new(&rel_path);
        match git_listed_files(&canonical_root, &target_dir) {
            Some(listed) => {
                if listed.contains(&rel_path) {
                    if is_synced_git_path_with(&rel_path, &sync_include) {
                        if let Some((content, is_exec, meta)) =
                            read_file_or_contained_symlink(&target_dir, &canonical_root, &rel_path)?
                        {
                            if fits_sync(&rel_path, &meta) {
                                deltas.push(FileDelta {
                                    relative_path: rel_path,
                                    content: Some(content),
                                    is_executable: is_exec,
                                });
                            } else if !is_non_overridable_sync_path(rel) {
                                omitted.push(UploadExclusion {
                                    path: rel_path,
                                    reason: UploadExclusionReason::OverSizeLimit,
                                });
                            }
                        }
                    } else if !is_non_overridable_sync_path(rel) {
                        omitted.push(UploadExclusion {
                            path: rel_path,
                            reason: UploadExclusionReason::OutputOrDataDirectory,
                        });
                    }
                }
            }
            None => {
                if !is_non_git_ignored(&canonical_root, rel) {
                    if is_relevant_code_or_manifest_file(&rel_path)
                        || sync_include.is_included(&rel_path)
                    {
                        if let Some((content, is_exec, meta)) =
                            read_file_or_contained_symlink(&target_dir, &canonical_root, &rel_path)?
                        {
                            if meta.len() > MAX_FILE_SIZE
                                || (rel_path.ends_with(".json")
                                    && meta.len() > MAX_JSON_CONFIG_SIZE)
                            {
                                if !is_non_overridable_sync_path(rel) {
                                    omitted.push(UploadExclusion {
                                        path: rel_path,
                                        reason: UploadExclusionReason::OverSizeLimit,
                                    });
                                }
                            } else {
                                deltas.push(FileDelta {
                                    relative_path: rel_path,
                                    content: Some(content),
                                    is_executable: is_exec,
                                });
                            }
                        }
                    } else if !is_non_overridable_sync_path(rel) && !is_synced_git_path(&rel_path) {
                        omitted.push(UploadExclusion {
                            path: rel_path,
                            reason: UploadExclusionReason::OutputOrDataDirectory,
                        });
                    }
                }
            }
        }
        return Ok((deltas, omitted));
    }

    if let Some(listed) = git_listed_files(&canonical_root, &target_dir) {
        for rel_path in listed {
            if !is_synced_git_path_with(&rel_path, &sync_include) {
                if !is_non_overridable_sync_path(Path::new(&rel_path))
                    && canonical_root.join(&rel_path).is_file()
                {
                    omitted.push(UploadExclusion {
                        path: rel_path,
                        reason: UploadExclusionReason::OutputOrDataDirectory,
                    });
                }
                continue;
            }
            let full_path = canonical_root.join(&rel_path);
            let Some((content, is_executable, meta)) =
                read_file_or_contained_symlink(&full_path, &canonical_root, &rel_path)?
            else {
                continue;
            };
            if !fits_sync(&rel_path, &meta) {
                if !is_non_overridable_sync_path(Path::new(&rel_path)) {
                    omitted.push(UploadExclusion {
                        path: rel_path,
                        reason: UploadExclusionReason::OverSizeLimit,
                    });
                }
                continue;
            }
            deltas.push(FileDelta {
                relative_path: rel_path,
                content: Some(content),
                is_executable,
            });
        }
        omitted.sort_by(|a, b| a.path.cmp(&b.path));
        omitted.dedup_by(|a, b| a.path == b.path);
        return Ok((deltas, omitted));
    }

    walk_dir(
        &target_dir,
        &canonical_root,
        &sync_include,
        &mut deltas,
        &mut omitted,
    )?;
    omitted.sort_by(|a, b| a.path.cmp(&b.path));
    omitted.dedup_by(|a, b| a.path == b.path);
    Ok((deltas, omitted))
}

/// Collect dirty, modified, untracked, and deleted files in a workspace directory.
/// When in a git repository or worktree, uses `git status --porcelain -uall` for sub-10ms discovery.
/// Uses lightweight memoization cache so files that were already synced and unchanged are not re-read or re-sent.
pub fn collect_dirty_files(root: &Path) -> Result<Vec<FileDelta>> {
    // The complete dirty/untracked set, every time. A gateway session keeps these files as its
    // private overlay, so each new session must announce all of them; a persistent
    // "already sent" cache would silently drop them for the second connection onwards.
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    collect_git_dirty_files(&canonical_root, false)
}

/// Like [`collect_dirty_files`] but skips files whose mtime/size/hash watermark is already
/// recorded in the persistent per-worktree sync state. Only correct against a server that keeps
/// previously synced files, i.e. the disk-backed base workspace, not session overlays.
pub fn collect_dirty_files_incremental(root: &Path) -> Result<Vec<FileDelta>> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    collect_git_dirty_files(&canonical_root, true)
}

pub(crate) fn is_binary_or_media_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".tar")
        || lower.ends_with(".gz")
        || lower.ends_with(".bin")
        || lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".pdf")
        || lower.ends_with(".wasm")
        || lower.ends_with(".so")
        || lower.ends_with(".dylib")
        || lower.ends_with(".a")
        || lower.ends_with(".o")
        || lower.ends_with(".exe")
        || lower.ends_with(".hprof")
        || lower.ends_with(".mp4")
        || lower.ends_with(".mov")
        || lower.ends_with(".pyc")
        || lower.ends_with(".db")
        || lower.ends_with(".sqlite")
}

/// Whether the non-Git walk prunes the entry at `path` (named `name`) as an output or data
/// tree. `path` is the walked path, so a checkout under a `src/` directory keeps data trees.
pub(crate) fn walk_prunes(path: &Path, name: &str) -> bool {
    if matches!(
        name,
        "vendor" | "results" | "samples" | "artifacts" | "dogfood-output" | "state"
    ) {
        return true;
    }
    let is_under_code = path.components().any(|c| {
        if let std::path::Component::Normal(p) = c {
            matches!(p.to_string_lossy().as_ref(), "crates" | "packages" | "src")
        } else {
            false
        }
    });
    !is_under_code
        && matches!(
            name,
            "research"
                | "benchmarks"
                | "benchmark"
                | "data"
                | "dataset"
                | "datasets"
                | "corpus"
                | "traces"
        )
}

pub(crate) fn walk_dir(
    target_dir: &Path,
    canonical_root: &Path,
    sync_include: &SyncInclude,
    deltas: &mut Vec<FileDelta>,
    omitted: &mut Vec<UploadExclusion>,
) -> Result<()> {
    if is_filesystem_root(canonical_root) {
        anyhow::bail!(
            "refusing to scan filesystem root: {}",
            canonical_root.display()
        );
    }
    let mut total_bytes: usize = deltas
        .iter()
        .map(|d| d.content.as_ref().map_or(0, |c| c.len()))
        .sum();
    let config = crate::config::load_config(canonical_root);
    let snapshot = crate::config::IgnoreSnapshot::build(canonical_root, &config.watch);
    let sync_for_filter = sync_include.clone();
    let root_for_filter = canonical_root.to_path_buf();
    let pruned_dirs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let pruned_for_filter = Arc::clone(&pruned_dirs);

    let mut builder = ignore::WalkBuilder::new(target_dir);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .max_filesize(Some(MAX_FILE_SIZE))
        .filter_entry(move |entry| {
            let path = entry.path();
            if snapshot.is_ignored(path) {
                return false;
            }
            let name = entry.file_name().to_string_lossy();
            if !walk_prunes(path, &name) {
                return true;
            }
            let rel_dir = path.strip_prefix(&root_for_filter).unwrap_or(path);
            if sync_for_filter.may_contain_included(rel_dir) {
                return true;
            }
            if name != "vendor"
                && !is_non_overridable_sync_path(rel_dir)
                && std::fs::read_dir(path).ok().is_some_and(|rd| {
                    rd.flatten()
                        .any(|e| !e.file_name().to_string_lossy().starts_with('.'))
                })
            {
                pruned_for_filter
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(format!("{}/", rel_dir.to_string_lossy()));
            }
            false
        });

    for result in builder.build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            continue;
        }
        let path = entry.path();
        let rel_path = path
            .strip_prefix(canonical_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        if !is_relevant_code_or_manifest_file(&rel_path) && !sync_include.is_included(&rel_path) {
            if !is_non_overridable_sync_path(Path::new(&rel_path)) && !is_synced_git_path(&rel_path)
            {
                omitted.push(UploadExclusion {
                    path: rel_path,
                    reason: UploadExclusionReason::OutputOrDataDirectory,
                });
            }
            continue;
        }

        let Some((content, is_executable, meta)) =
            read_file_or_contained_symlink(path, canonical_root, &rel_path)?
        else {
            continue;
        };

        if rel_path.ends_with(".json") && meta.len() > MAX_JSON_CONFIG_SIZE {
            if !is_non_overridable_sync_path(Path::new(&rel_path)) {
                omitted.push(UploadExclusion {
                    path: rel_path,
                    reason: UploadExclusionReason::OverSizeLimit,
                });
            }
            continue;
        }

        total_bytes += content.len();
        deltas.push(FileDelta {
            relative_path: rel_path,
            content: Some(content),
            is_executable,
        });
        if deltas.len() > MAX_NON_GIT_WORKSPACE_FILES || total_bytes > MAX_NON_GIT_WORKSPACE_BYTES {
            anyhow::bail!(
                "non-git workspace exceeds safety limits ({} files, {} bytes); use a git checkout or specify a narrower subpath",
                deltas.len(),
                total_bytes
            );
        }
    }

    for dir in pruned_dirs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
    {
        omitted.push(UploadExclusion {
            path: dir,
            reason: UploadExclusionReason::OutputOrDataDirectory,
        });
    }
    Ok(())
}
