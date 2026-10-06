/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(crate) struct VisitedDirs {
    #[cfg(unix)]
    dev_ino: std::collections::HashSet<(u64, u64)>,
    #[cfg(not(unix))]
    canonical: std::collections::HashSet<PathBuf>,
}

impl VisitedDirs {
    pub(crate) fn insert(&mut self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(meta) = fs::metadata(path) {
                if !meta.is_dir() {
                    return false;
                }
                return self.dev_ino.insert((meta.dev(), meta.ino()));
            }
            false
        }
        #[cfg(not(unix))]
        {
            if let Ok(canon) = path.canonicalize() {
                if canon.is_dir() {
                    return self.canonical.insert(canon);
                }
            }
            false
        }
    }
}

/// Builds the set of canonical approved roots from which symlinks may be safely dereferenced.
/// Includes the project root(s) and any validated pnpm virtual/global stores.
pub fn build_approved_roots(roots: &[&Path]) -> Vec<PathBuf> {
    let mut approved = Vec::new();
    for r in roots {
        if let Ok(c) = r.canonicalize() {
            approved.push(c);
        } else {
            approved.push(r.to_path_buf());
        }
    }

    // Include PNPM_HOME / global pnpm store if valid
    if let Some(pnpm_home) = std::env::var_os("PNPM_HOME") {
        let p = PathBuf::from(pnpm_home);
        if let Ok(c) = p.canonicalize() {
            approved.push(c);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home_path = PathBuf::from(home);
        for pnpm_sub in &[".local/share/pnpm", ".pnpm-store", "Library/pnpm"] {
            let candidate = home_path.join(pnpm_sub);
            if candidate.is_dir() {
                if let Ok(c) = candidate.canonicalize() {
                    approved.push(c);
                }
            }
        }
    }

    approved
}

/// Finds the enclosing project root by searching parent directories for standard manifests.
pub fn find_enclosing_project_root(path: &Path) -> PathBuf {
    let mut current = if path.is_file() {
        path.parent()
    } else {
        Some(path)
    };
    let mut candidate = None;
    while let Some(dir) = current {
        if dir.join("package.json").is_file()
            || dir.join("tsconfig.json").is_file()
            || dir.join("pnpm-workspace.yaml").is_file()
            || dir.join(".git").exists()
        {
            candidate = Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    candidate.unwrap_or_else(|| {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.to_path_buf())
    })
}

/// Resolves a path to its canonical target and returns it if it resides within an approved root (#836).
pub fn approved_target(target: &Path, approved_roots: &[PathBuf]) -> Option<PathBuf> {
    let canon = target.canonicalize().ok()?;
    if approved_roots.iter().any(|root| canon.starts_with(root)) {
        Some(canon)
    } else {
        None
    }
}

/// Checks whether a symlink's target canonical path is inside an approved project or package root.
pub fn is_target_approved(target: &Path, approved_roots: &[PathBuf]) -> bool {
    approved_target(target, approved_roots).is_some()
}
