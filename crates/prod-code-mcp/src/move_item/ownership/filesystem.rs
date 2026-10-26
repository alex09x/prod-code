/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Collapse lexical components only after proving that traversal cannot cross a
/// link. Untraversed linked declarations remain evidence for alias checks.
pub(super) fn resolve(root: &Path, path: &Path) -> PathBuf {
    if no_links(root, path).is_ok() {
        super::normalize(path)
    } else {
        path.to_path_buf()
    }
}

/// A directory link aliases its entire physical subtree, even when graph discovery
/// stops at an entry file several levels above the file being edited. Inspect link
/// components only; never enumerate their targets (which may contain cycles).
pub(super) fn linked_coverage(root: &Path, path: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let mut coverage = Vec::new();
    for part in path.ancestors().take_while(|p| p.starts_with(root)) {
        if std::fs::symlink_metadata(part).is_ok_and(|m| m.is_symlink()) {
            let target = part.canonicalize()?;
            let directory = target.is_dir();
            coverage.push((target, directory));
        }
    }
    // A link followed by `..` can resolve outside the linked directory. Record
    // that exact file as well as the directory coverage, without walking either.
    if !coverage.is_empty() {
        if let Ok(target) = path.canonicalize() {
            let directory = target.is_dir();
            coverage.push((target, directory));
        }
    }
    Ok(coverage)
}

pub(crate) fn path_literal(base: &Path, target: &Path) -> Result<String> {
    let common = base
        .ancestors()
        .find(|p| target.starts_with(p))
        .ok_or_else(|| anyhow::anyhow!("module paths have no common checkout"))?;
    let mut relative = PathBuf::new();
    for _ in base.strip_prefix(common)?.components() {
        relative.push("..");
    }
    relative.push(target.strip_prefix(common)?);
    Ok(serde_json::to_string(&relative.to_string_lossy())?)
}

/// Whole-file workspace edits cannot represent a link-preserving rename. Refuse this
/// specific filesystem plan before reading through a link, even with force enabled.
pub(crate) fn no_links(root: &Path, path: &Path) -> Result<()> {
    anyhow::ensure!(
        super::normalize(path).starts_with(root),
        "move path leaves checkout; nothing was written"
    );
    // Inspect the original traversal before removing parent components. Looking
    // only at the normalized spelling can erase a link and inspect a decoy file.
    for part in path.ancestors().take_while(|p| p.starts_with(root)) {
        if let Ok(meta) = std::fs::symlink_metadata(part) {
            anyhow::ensure!(
                !meta.is_symlink(),
                "link-preserving moves are unsupported for {}; nothing was written",
                part.display()
            );
        }
    }
    Ok(())
}

pub(crate) fn files(root: &Path, dir: &Path) -> Result<Vec<PathBuf>> {
    no_links(root, dir)?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        anyhow::ensure!(
            !kind.is_symlink(),
            "link-preserving moves are unsupported for {}; nothing was written",
            path.display()
        );
        if kind.is_dir() {
            out.extend(files(root, &path)?);
        } else {
            anyhow::ensure!(
                kind.is_file(),
                "unsupported filesystem entry {}; nothing was written",
                path.display()
            );
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}
