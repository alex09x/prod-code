/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::cache::{load_sync_cache, save_sync_cache};
use crate::sync::entry::sync_file_entry;
use crate::sync::relevance::is_relevant_code_or_manifest_file;
use crate::sync::scan::read_regular_file_secure;
use crate::sync::types::{MAX_FILE_SIZE, MAX_JSON_CONFIG_SIZE, SyncCache};
use anyhow::Result;
use prod_code_protocol::FileDelta;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

/// Which of `paths` git lists (tracked, or untracked and not ignored). An ignored file is never
/// sent, even when the gateway names it.
pub(crate) fn git_listed_paths(root: &Path, paths: &[String]) -> HashSet<String> {
    if paths.is_empty() {
        return HashSet::new();
    }
    let Ok(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "--literal-pathspecs",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
        ])
        .args(paths)
        .output()
    else {
        return HashSet::new();
    };
    if !output.status.success() {
        return HashSet::new();
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect()
}

pub(crate) fn git_head(root: &Path) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("git rev-parse HEAD non-zero exit");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Paths changed since `base` (committed, dirty and untracked) mapped to "deleted", plus the
/// set of paths that are currently dirty or untracked.
pub(crate) fn changed_paths(
    root: &Path,
    base: Option<&str>,
    head: &str,
) -> Result<(BTreeMap<String, bool>, BTreeSet<String>)> {
    let mut paths = BTreeMap::new();
    // A recorded base can disappear after a rebase, amend or gc; fall back to the full tracked
    // tree instead of failing the sync, since the watermarks still filter unchanged files.
    // When HEAD has not moved since the recorded base there are no committed changes to
    // list, so the diff (the most expensive git call of the round) is skipped entirely.
    let diff_from_base = match base {
        Some(base) if base == head => Some(Vec::new()),
        Some(base) if !base.is_empty() => {
            git_output(root, ["diff", "--name-status", "-z", base]).ok()
        }
        _ => None,
    };
    match diff_from_base {
        Some(output) => parse_name_status(&output, &mut paths),
        None => {
            let output = git_output(root, ["ls-files", "-z"])?;
            for path in output
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
            {
                paths.insert(String::from_utf8_lossy(path).to_string(), false);
            }
        }
    }

    let output = git_output(root, ["status", "--porcelain=v1", "-z", "-uall"])?;
    let mut dirty = BTreeMap::new();
    parse_porcelain_status(&output, &mut dirty);
    let dirty_set: BTreeSet<String> = dirty.keys().cloned().collect();
    paths.extend(dirty);
    Ok((paths, dirty_set))
}

pub(crate) fn git_output<const N: usize>(root: &Path, args: [&str; N]) -> Result<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        anyhow::bail!("git command non-zero exit");
    }
    Ok(output.stdout)
}

pub(crate) fn parse_name_status(output: &[u8], paths: &mut BTreeMap<String, bool>) {
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(status) = fields.next() {
        let status = String::from_utf8_lossy(status);
        let Some(path) = fields.next() else { break };
        if status.starts_with('R') {
            let Some(new_path) = fields.next() else { break };
            paths.insert(String::from_utf8_lossy(path).to_string(), true);
            paths.insert(String::from_utf8_lossy(new_path).to_string(), false);
        } else if status.starts_with('C') {
            let Some(new_path) = fields.next() else { break };
            paths.insert(String::from_utf8_lossy(new_path).to_string(), false);
        } else {
            paths.insert(
                String::from_utf8_lossy(path).to_string(),
                status.starts_with('D'),
            );
        }
    }
}

pub(crate) fn parse_porcelain_status(output: &[u8], paths: &mut BTreeMap<String, bool>) {
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let status = &entry[..2];
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        if status.contains(&b'R') {
            let Some(old_path) = fields.next() else { break };
            paths.insert(String::from_utf8_lossy(old_path).to_string(), true);
        } else if status.contains(&b'C') {
            let Some(_) = fields.next() else { break };
        }
        paths.insert(path, status.contains(&b'D'));
    }
}

/// The files git lists under `dir` — tracked, and untracked but not ignored — relative to `root`,
/// or `None` when `root` is not in a git checkout.
pub(crate) fn git_listed_files(root: &Path, dir: &Path) -> Option<Vec<String>> {
    let rel = dir.strip_prefix(root).ok()?;
    let spec = if rel.as_os_str().is_empty() {
        ".".to_string()
    } else {
        rel.to_string_lossy().into_owned()
    };
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
        ])
        .arg(spec)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut files: Vec<String> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect();
    files.sort();
    files.dedup();
    Some(files)
}

pub(crate) fn collect_git_dirty_files(root: &Path, use_cache: bool) -> Result<Vec<FileDelta>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("status")
        .arg("--porcelain")
        .arg("-uall")
        .output()?;

    if !output.status.success() {
        anyhow::bail!("git status non-zero exit");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut deltas = Vec::new();
    let mut cache = if use_cache {
        load_sync_cache(root)
    } else {
        SyncCache::default()
    };
    let mut cache_modified = false;

    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        let raw_path = line[3..].trim();

        let (is_delete, rel_path, old_path) = if status.contains('D') {
            (true, raw_path, None)
        } else if let Some((old_p, new_p)) = raw_path.split_once(" -> ") {
            (
                false,
                new_p.trim().trim_matches('"'),
                Some(old_p.trim().trim_matches('"')),
            )
        } else {
            (false, raw_path.trim_matches('"'), None)
        };

        if let Some(old) = old_path
            && !old.starts_with(".git")
        {
            deltas.push(FileDelta {
                relative_path: old.to_string(),
                content: None,
                is_executable: false,
            });
            if cache.files.remove(old).is_some() {
                cache_modified = true;
            }
        }

        if !is_relevant_code_or_manifest_file(rel_path) {
            continue;
        }

        let full_path = root.join(rel_path);
        if is_delete {
            deltas.push(FileDelta {
                relative_path: rel_path.to_string(),
                content: None,
                is_executable: false,
            });
            if cache.files.remove(rel_path).is_some() {
                cache_modified = true;
            }
        } else if let Ok(sym_meta) = full_path.symlink_metadata() {
            if sym_meta.file_type().is_symlink() {
                continue;
            }
            let size = sym_meta.len();
            if size > MAX_FILE_SIZE {
                continue;
            }
            if rel_path.ends_with(".json") && size > MAX_JSON_CONFIG_SIZE {
                continue;
            }

            let Some((content, is_executable)) = read_regular_file_secure(&full_path, root)? else {
                continue;
            };
            let entry = sync_file_entry(&sym_meta, &content);
            if cache.files.get(rel_path) == Some(&entry) {
                // File was already synced and has not changed.
                continue;
            }
            deltas.push(FileDelta {
                relative_path: rel_path.to_string(),
                content: Some(content),
                is_executable,
            });
            cache.files.insert(rel_path.to_string(), entry);
            cache_modified = true;
        }
    }

    if use_cache && cache_modified {
        save_sync_cache(root, &cache);
    }

    Ok(deltas)
}
