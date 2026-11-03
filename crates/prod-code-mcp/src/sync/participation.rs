/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Sync participation and omission diagnostics (#1813, #1836).
//!
//! Ordinary sync deliberately keeps output and data trees out unless opted in via `[sync] include`
//! in `prod-code.toml`. This module classifies both written-back files after an exec (#1813) and
//! unignored local files omitted during pre-flight sync (#1836), applying the effective Git or
//! walk rules without a full-tree scan.

use crate::config::{IgnoreSnapshot, WatchConfig, is_baseline_ignored, load_config};
use crate::sync::entry::fits_sync;
use crate::sync::filter_path::{SyncPathFilter, is_synced_git_path, is_synced_git_path_with};
use crate::sync::git::{git_head, git_listed_paths};
use crate::sync::include::{SyncInclude, is_non_overridable_sync_path};
use crate::sync::relevance::is_relevant_code_or_manifest_file;
use crate::sync::scan::walk_prunes;
use crate::sync::types::{MAX_FILE_SIZE, MAX_JSON_CONFIG_SIZE, SyncFileEntry};
use ignore::IncrementalIgnore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

#[cfg(test)]
#[path = "participation_tests.rs"]
mod tests;

/// Why a checkout file is omitted from mirror upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadExclusionReason {
    /// Git does not list it, or (non-Git workspaces) an ignore rule matches it.
    GitIgnored,
    /// A directory above it matches `[watch] ignore` in `prod-code.toml` (non-Git workspaces).
    WatchIgnored,
    /// Under an output, cache or data directory (`results/`, `artifacts/`, `site/data/`, `target/`…).
    OutputOrDataDirectory,
    /// A hidden path, or (non-Git workspaces) an extension outside the upload allowlist.
    HiddenOrExtension,
    /// Larger than the per-file upload size cap.
    OverSizeLimit,
}

impl UploadExclusionReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::GitIgnored => "git-ignored",
            Self::WatchIgnored => "watch-ignored",
            Self::OutputOrDataDirectory => "output/data directory",
            Self::HiddenOrExtension => "hidden or unlisted extension",
            Self::OverSizeLimit => "over size limit",
        }
    }
}

/// One checkout file left out of mirror upload, paired with the rule that excluded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadExclusion {
    pub path: String,
    pub reason: UploadExclusionReason,
}

const GIT_PATHSPEC_CHUNK: usize = 512;
const WARNING_PATHS: usize = 5;

/// Classify `paths` (checkout-relative, as written back) with the effective upload rules of
/// `root`: a Git checkout uses Git's listing, [`is_synced_git_path_with`] and [`fits_sync`]; any
/// other directory uses the walk's hidden, ignore, pruning, size, relevance and `[sync] include`
/// rules. Deleted or non-regular paths are not classified. Returns only the excluded paths.
pub fn classify_upload_exclusions(root: &Path, paths: &[String]) -> Vec<UploadExclusion> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let present: Vec<(String, std::fs::Metadata)> = paths
        .iter()
        .filter(|p| {
            let rel = Path::new(p.as_str());
            !rel.is_absolute() && rel.components().all(|c| matches!(c, Component::Normal(_)))
        })
        .filter_map(|p| {
            let meta = std::fs::metadata(root.join(p)).ok()?;
            meta.is_file().then(|| (p.clone(), meta))
        })
        .collect();
    if present.is_empty() {
        return Vec::new();
    }
    let reasons = if git_head(&root).is_ok() {
        classify_git(&root, &present)
    } else {
        classify_walk(&root, &present)
    };
    present
        .into_iter()
        .zip(reasons)
        .filter_map(|((path, _), reason)| reason.map(|reason| UploadExclusion { path, reason }))
        .collect()
}

fn classify_git(
    root: &Path,
    present: &[(String, std::fs::Metadata)],
) -> Vec<Option<UploadExclusionReason>> {
    let sync_include = SyncInclude::load(root);
    let names: Vec<String> = present.iter().map(|(p, _)| p.clone()).collect();
    let mut listed = HashSet::new();
    for chunk in names.chunks(GIT_PATHSPEC_CHUNK) {
        listed.extend(git_listed_paths(root, chunk));
    }
    present
        .iter()
        .map(|(path, meta)| {
            if !listed.contains(path) {
                Some(UploadExclusionReason::GitIgnored)
            } else if !is_synced_git_path_with(path, &sync_include) {
                Some(directory_or_hidden(Path::new(path)))
            } else if !fits_sync(path, meta) {
                Some(UploadExclusionReason::OverSizeLimit)
            } else {
                None
            }
        })
        .collect()
}

fn classify_walk(
    root: &Path,
    present: &[(String, std::fs::Metadata)],
) -> Vec<Option<UploadExclusionReason>> {
    let config = load_config(root);
    let sync_include = SyncInclude::from_patterns(root, &config.sync.include);
    let with_watch = IgnoreSnapshot::build(root, &config.watch);
    let git_only = IgnoreSnapshot::build(
        root,
        &WatchConfig {
            ignore: Vec::new(),
            use_gitignore: config.watch.use_gitignore,
        },
    );
    let mut walk_ignore = build_walk_ignore(root);
    present
        .iter()
        .map(|(path, meta)| {
            let rel = Path::new(path);
            if let Some(reason) = pruned_ancestor(
                root,
                rel,
                &with_watch,
                &git_only,
                &sync_include,
                walk_ignore.as_mut(),
            ) {
                Some(reason)
            } else if is_hidden(rel) {
                Some(UploadExclusionReason::HiddenOrExtension)
            } else if walk_ignore
                .as_mut()
                .is_some_and(|ig| ig.matched(rel, false).is_ignore())
            {
                Some(UploadExclusionReason::GitIgnored)
            } else if meta.len() > MAX_FILE_SIZE
                || (path.ends_with(".json") && meta.len() > MAX_JSON_CONFIG_SIZE)
            {
                Some(UploadExclusionReason::OverSizeLimit)
            } else if !is_relevant_code_or_manifest_file(path) && !sync_include.is_included(path) {
                Some(if is_synced_git_path(path) {
                    UploadExclusionReason::HiddenOrExtension
                } else {
                    directory_or_hidden(rel)
                })
            } else {
                None
            }
        })
        .collect()
}

pub(crate) fn is_non_git_ignored(root: &Path, rel: &Path) -> bool {
    if is_hidden(rel) {
        return true;
    }
    let config = load_config(root);
    let with_watch = IgnoreSnapshot::build(root, &config.watch);
    let mut walked = root.to_path_buf();
    for dir in rel.parent().into_iter().flat_map(Path::components) {
        walked.push(dir);
        if with_watch.is_ignored(&walked) {
            return true;
        }
    }
    build_walk_ignore(root).is_some_and(|mut ig| ig.matched(rel, false).is_ignore())
}

fn build_walk_ignore(root: &Path) -> Option<IncrementalIgnore> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true);
    builder.build_matchers().into_iter().next()
}

fn pruned_ancestor(
    root: &Path,
    rel: &Path,
    with_watch: &IgnoreSnapshot,
    git_only: &IgnoreSnapshot,
    sync_include: &SyncInclude,
    mut walk_ignore: Option<&mut IncrementalIgnore>,
) -> Option<UploadExclusionReason> {
    let mut walked = root.to_path_buf();
    let mut prefix = std::path::PathBuf::new();
    for dir in rel.parent()?.components() {
        walked.push(dir);
        prefix.push(dir);
        let name = dir.as_os_str().to_string_lossy();
        if name.starts_with('.') {
            return Some(UploadExclusionReason::HiddenOrExtension);
        }
        if walk_ignore
            .as_deref_mut()
            .is_some_and(|ig| ig.matched(&prefix, true).is_ignore())
        {
            return Some(UploadExclusionReason::GitIgnored);
        }
        if with_watch.is_ignored(&walked) {
            return Some(if is_baseline_ignored(&prefix) {
                UploadExclusionReason::OutputOrDataDirectory
            } else if git_only.is_ignored(&walked) {
                UploadExclusionReason::GitIgnored
            } else {
                UploadExclusionReason::WatchIgnored
            });
        }
        if walk_prunes(&walked, &name) && !sync_include.may_contain_included(&prefix) {
            return Some(UploadExclusionReason::OutputOrDataDirectory);
        }
    }
    None
}

fn is_hidden(rel: &Path) -> bool {
    rel.components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
}

fn directory_or_hidden(rel: &Path) -> UploadExclusionReason {
    if is_hidden(rel) {
        UploadExclusionReason::HiddenOrExtension
    } else {
        UploadExclusionReason::OutputOrDataDirectory
    }
}

/// Collect Git-listed checkout files that remain absent from `synced_files` because output/data
/// directory rules or per-file size caps excluded them (#1836).
pub(crate) fn collect_git_sync_omissions<'a>(
    canonical_root: &Path,
    candidates: impl IntoIterator<Item = &'a String>,
    synced_files: &HashMap<String, SyncFileEntry>,
    filter: &SyncPathFilter,
    sync_include: &SyncInclude,
) -> Vec<UploadExclusion> {
    let mut omitted = Vec::new();
    for rel in candidates {
        if synced_files.contains_key(rel)
            || !filter.includes(rel)
            || is_non_overridable_sync_path(Path::new(rel))
        {
            continue;
        }
        let full_path = canonical_root.join(rel);
        let Ok(meta) = std::fs::metadata(&full_path) else {
            continue;
        };
        if !meta.is_file()
            || !std::fs::canonicalize(&full_path)
                .is_ok_and(|canon| canon.starts_with(canonical_root))
        {
            continue;
        }
        if !is_synced_git_path_with(rel, sync_include) {
            omitted.push(UploadExclusion {
                path: rel.clone(),
                reason: UploadExclusionReason::OutputOrDataDirectory,
            });
        } else if !fits_sync(rel, &meta) {
            omitted.push(UploadExclusion {
                path: rel.clone(),
                reason: UploadExclusionReason::OverSizeLimit,
            });
        }
    }
    omitted.sort_by(|a, b| a.path.cmp(&b.path));
    omitted.dedup_by(|a, b| a.path == b.path);
    omitted
}

fn format_exclusion_list(excluded: &[UploadExclusion]) -> String {
    let mut listed = excluded
        .iter()
        .take(WARNING_PATHS)
        .map(|e| format!("{} ({})", e.path, e.reason.label()))
        .collect::<Vec<_>>()
        .join(", ");
    if excluded.len() > WARNING_PATHS {
        listed.push_str(&format!(", and {} more", excluded.len() - WARNING_PATHS));
    }
    listed
}

/// Compact one-line warning naming written-back files that stay out of future uploads (#1813).
pub fn upload_exclusion_warning(excluded: &[UploadExclusion]) -> Option<String> {
    if excluded.is_empty() {
        return None;
    }
    let listed = format_exclusion_list(excluded);
    Some(format!(
        "upload warning: {} written-back file(s) stay in this checkout and are not uploaded to other nodes: {listed}. \
         For inputs another node needs, add `[sync] include = [\"...\"]` in prod-code.toml, keep them under an unignored fixtures/, testdata/ or tests/fixtures/ path in a Git checkout (within the size limit), or pin a Git checkout to one node (--pin-node)",
        excluded.len()
    ))
}

/// Compact one-line warning naming unignored local files omitted from the remote mirror (#1836).
pub fn sync_omission_warning(omitted: &[UploadExclusion]) -> Option<String> {
    if omitted.is_empty() {
        return None;
    }
    let listed = format_exclusion_list(omitted);
    Some(format!(
        "sync warning: {} unignored local file(s) omitted from the remote mirror: {listed}. \
         Add `[sync] include = [\"...\"]` in prod-code.toml to upload required inputs within the size limit, or add an ignore rule",
        omitted.len()
    ))
}
