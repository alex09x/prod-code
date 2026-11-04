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

/// Returns Git-tracked symlinks that belong to the workspace sync filter.
pub(crate) fn tracked_symlink_manifest(
    root: &Path,
) -> Vec<prod_code_protocol::messages::sync::SyncSymlink> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let Ok(output) = super::git::git_output(&canonical_root, ["ls-files", "-s", "-z"]) else {
        return Vec::new();
    };
    let include = super::include::SyncInclude::load(&canonical_root);
    output
        .split(|byte| *byte == 0)
        .filter_map(|record| {
            if !record.starts_with(b"120000 ") {
                return None;
            }
            let tab = record.iter().position(|byte| *byte == b'\t')?;
            let relative_path = std::str::from_utf8(&record[tab + 1..]).ok()?;
            if !super::filter_path::is_synced_git_path_with(relative_path, &include) {
                return None;
            }
            let target = std::fs::read_link(canonical_root.join(relative_path))
                .ok()?
                .into_os_string()
                .into_string()
                .ok()?;
            Some(prod_code_protocol::messages::sync::SyncSymlink {
                relative_path: relative_path.to_string(),
                target,
            })
        })
        .collect()
}

pub(crate) fn is_alias_path(
    relative_path: &str,
    symlinks: &[prod_code_protocol::messages::sync::SyncSymlink],
) -> bool {
    symlinks.iter().any(|link| {
        relative_path == link.relative_path
            || relative_path
                .strip_prefix(&link.relative_path)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::process::Command;

    #[test]
    fn tracked_dangling_symlink_is_included_in_full_manifest() {
        let root = tempfile::tempdir().expect("temporary Git workspace");
        let link = root.path().join("swift/Sources/App/_bindings");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink("../../../target/bindings", &link).unwrap();
        assert!(
            Command::new("git")
                .args(["-C", root.path().to_str().unwrap(), "init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args([
                    "-C",
                    root.path().to_str().unwrap(),
                    "add",
                    "--",
                    "swift/Sources/App/_bindings"
                ])
                .status()
                .unwrap()
                .success()
        );

        let links = tracked_symlink_manifest(root.path());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].relative_path, "swift/Sources/App/_bindings");
        assert_eq!(links[0].target, "../../../target/bindings");
    }
}
