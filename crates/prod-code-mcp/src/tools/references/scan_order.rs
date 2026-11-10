/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The order the multi-target references scan reads the checkout's files in.

use std::path::{Path, PathBuf};

/// The files to scan, nearest first: the declaring file, then the files beside it, then the
/// rest in their walk order. A common name (`present`, `update`) is written in many files, each
/// asked within the budget, and in walk order the declaring file, where a `private` member's
/// only uses are, could come after the budget ran out (#2008).
///
/// The declaring file goes first only when it is one of `files`: a declaration in a dependency
/// or an excluded directory is not a file of the checkout, and its own uses are not the
/// checkout's references.
pub(crate) fn scan_order(declaration: &Path, files: impl Iterator<Item = PathBuf>) -> Vec<PathBuf> {
    let parent = declaration.parent();
    let mut declared_here = false;
    let mut files: Vec<PathBuf> = files
        .filter(|path| {
            let same = is_same_file(path, declaration);
            declared_here |= same;
            !same
        })
        .collect();
    // A stable sort keeps the walk order within each group.
    files.sort_by_key(|path| path.parent() != parent);
    if declared_here {
        files.insert(0, declaration.to_path_buf());
    }
    files
}

/// The files a scan reads, or `None` when it can read none. Out of time before the scan starts,
/// only a Swift declaring file of this checkout can still be read (from its text, #2008), so the
/// walk stops at it instead of ordering every file.
pub(crate) fn candidates(
    declaration: &Path,
    declares_swift: bool,
    expired: bool,
    mut sources: impl Iterator<Item = PathBuf>,
) -> Option<Vec<PathBuf>> {
    if !expired {
        return Some(scan_order(declaration, sources));
    }
    (declares_swift && sources.any(|path| is_same_file(&path, declaration)))
        .then(|| vec![declaration.to_path_buf()])
}

/// Whether `path` is the declaring file, however it is spelled. Only a path with the same file
/// name is resolved on disk, so a walk of thousands of files costs no system call each.
pub(crate) fn is_same_file(path: &Path, declaration: &Path) -> bool {
    if path == declaration {
        return true;
    }
    if path.file_name() != declaration.file_name() {
        return false;
    }
    match (
        std::fs::canonicalize(path),
        std::fs::canonicalize(declaration),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
#[path = "multi_target_order_tests.rs"]
mod tests;
