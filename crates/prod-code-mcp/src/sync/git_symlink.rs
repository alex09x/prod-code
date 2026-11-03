/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Targeted membership checks for paths beneath git-tracked contained directory symlinks (#1813).
//!
//! Full syncs expand tracked directory symlinks to enumerate workspace files, while explicit path
//! queries (`git_listed_paths`) inspect only the candidate path's ancestor components (`lstat`)
//! and never call `read_dir` on the symlink target or unrelated directories.

use std::collections::{BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};

/// Adds any unlisted entries in `paths` that resolve to regular files inside a git-tracked
/// directory symlink (`mode 120000`) contained within `root`.
pub(crate) fn extend_listed_via_tracked_dir_symlinks(
    root: &Path,
    paths: &[String],
    listed: &mut HashSet<String>,
) {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut via_symlink: Vec<(&str, String)> = Vec::new();
    let mut ancestors: BTreeSet<String> = BTreeSet::new();
    for path in paths {
        if !listed.contains(path)
            && let Some(anc) = shallowest_symlink_ancestor(&canonical_root, path)
        {
            ancestors.insert(anc.clone());
            via_symlink.push((path.as_str(), anc));
        }
    }
    if ancestors.is_empty() {
        return;
    }
    let tracked = tracked_symlinks_among(&canonical_root, &ancestors);
    for (path, anc) in via_symlink {
        if tracked.contains(&anc) && is_contained_under_dir_symlink(&canonical_root, &anc, path) {
            listed.insert(path.to_string());
        }
    }
}

fn shallowest_symlink_ancestor(canonical_root: &Path, rel_path: &str) -> Option<String> {
    let parent = Path::new(rel_path).parent()?;
    let mut prefix = PathBuf::new();
    for comp in parent.components() {
        let Component::Normal(seg) = comp else {
            return None;
        };
        prefix.push(seg);
        if canonical_root
            .join(&prefix)
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return prefix.to_str().map(str::to_string);
        }
    }
    None
}

fn tracked_symlinks_among(canonical_root: &Path, candidates: &BTreeSet<String>) -> HashSet<String> {
    let args: Vec<&str> = candidates.iter().map(String::as_str).collect();
    let mut out = HashSet::new();
    for chunk in args.chunks(512) {
        let Ok(output) = std::process::Command::new("git")
            .arg("-C")
            .arg(canonical_root)
            .args(["--literal-pathspecs", "ls-files", "-s", "-z", "--"])
            .args(chunk)
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        for record in output.stdout.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            if record.starts_with(b"120000 ")
                && let Some(tab) = record.iter().position(|&b| b == b'\t')
            {
                out.insert(String::from_utf8_lossy(&record[tab + 1..]).into_owned());
            }
        }
    }
    out
}

fn is_contained_under_dir_symlink(canonical_root: &Path, link_rel: &str, target_rel: &str) -> bool {
    let link_path = canonical_root.join(link_rel);
    let Ok(sym_meta) = link_path.symlink_metadata() else {
        return false;
    };
    if !sym_meta.file_type().is_symlink() {
        return false;
    }
    let Ok(mut cur) = std::fs::canonicalize(&link_path) else {
        return false;
    };
    if !cur.is_dir() || !cur.starts_with(canonical_root) {
        return false;
    }
    let Ok(rest) = Path::new(target_rel).strip_prefix(link_rel.trim_end_matches('/')) else {
        return false;
    };
    let mut names = Vec::new();
    for comp in rest.components() {
        let Component::Normal(seg) = comp else {
            return false;
        };
        let Some(name_str) = seg.to_str() else {
            return false;
        };
        if matches!(name_str, ".git" | "target" | ".build" | "node_modules") {
            return false;
        }
        names.push(name_str);
    }
    let Some((&leaf, dirs)) = names.split_last() else {
        return false;
    };
    let mut visited = HashSet::new();
    visited.insert(cur.clone());
    for &dir_name in dirs {
        let next = cur.join(dir_name);
        let Ok(ft) = next.symlink_metadata().map(|m| m.file_type()) else {
            return false;
        };
        if !ft.is_dir() && !ft.is_symlink() {
            return false;
        }
        let Ok(canon_next) = std::fs::canonicalize(&next) else {
            return false;
        };
        if !canon_next.is_dir()
            || !canon_next.starts_with(canonical_root)
            || !visited.insert(canon_next.clone())
        {
            return false;
        }
        cur = canon_next;
    }
    let leaf_path = cur.join(leaf);
    let Ok(ft) = leaf_path.symlink_metadata().map(|m| m.file_type()) else {
        return false;
    };
    if ft.is_file() {
        true
    } else if ft.is_symlink() {
        std::fs::canonicalize(&leaf_path)
            .is_ok_and(|canon| canon.is_file() && canon.starts_with(canonical_root))
    } else {
        false
    }
}
