/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::git::git_listed_files;
use crate::sync::relevance::is_relevant_code_or_manifest_file;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub(crate) struct SyncPathFilter {
    relative: Option<PathBuf>,
}

impl SyncPathFilter {
    pub(crate) fn new(root: &Path, subpath: Option<&Path>) -> Result<Self> {
        let relative = match subpath {
            Some(path) => {
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    root.join(path)
                };
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                let canon_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
                let relative = path
                    .strip_prefix(&canon_root)
                    .or_else(|_| path.strip_prefix(root))?
                    .to_path_buf();
                if !path.exists()
                    && !git_listed_files(&canon_root, &path)
                        .is_some_and(|listed| !listed.is_empty())
                {
                    anyhow::bail!(
                        "sync path {:?} does not exist and is not a tracked deletion",
                        path
                    );
                }
                Some(relative)
            }
            None => None,
        };
        Ok(Self { relative })
    }

    pub(crate) fn includes(&self, relative_path: &str) -> bool {
        self.relative.as_ref().is_none_or(|filter| {
            Path::new(relative_path) == filter || Path::new(relative_path).starts_with(filter)
        })
    }
}

/// Returns true if `path` resolves to a filesystem root (such as `/` on Unix or drive root on Windows).
pub fn is_filesystem_root(path: &Path) -> bool {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canonical.parent().is_none() || canonical.as_os_str().is_empty() || canonical == Path::new("/")
}

/// Returns true if `path` is within a recognized test fixtures or testdata directory.
pub fn is_fixture_path(path: &Path) -> bool {
    path.components().any(|c| {
        if let std::path::Component::Normal(comp) = c {
            let s = comp.to_string_lossy();
            matches!(
                s.as_ref(),
                "fixtures" | "fixture" | "testdata" | "test_data" | "test-fixtures"
            )
        } else {
            false
        }
    })
}

/// Whether a file git lists — tracked, or untracked and not ignored — is mirrored to the gateway.
///
/// Everything git would check out goes, whatever its extension: test fixtures, snapshots,
/// `include_bytes!` data, `.github/` (#123). An extension allowlist made sense for a directory
/// walk, which cannot tell a fixture from a dataset; git can, because an ignored file is not
/// listed. What stays out is what [`is_relevant_code_or_manifest_file`] keeps out by directory —
/// data trees and build output — and `.git` itself. A `vendor/` directory git lists is a build
/// input (Go's `-mod=vendor`, a C library linked through cgo) and goes too (#313). The size
/// limits are applied where the file is read.
pub fn is_synced_git_path(rel_path: &str) -> bool {
    if is_relevant_code_or_manifest_file(rel_path) {
        return true;
    }
    let path = Path::new(rel_path);
    let mut under_code_dir = false;
    for component in path.parent().into_iter().flat_map(Path::components) {
        let std::path::Component::Normal(dir) = component else {
            continue;
        };
        let dir = dir.to_string_lossy();
        if dir == ".git" {
            return false;
        }
        if matches!(
            dir.as_ref(),
            "src"
                | "server"
                | "client"
                | "internal"
                | "pkg"
                | "cmd"
                | "api"
                | "Sources"
                | "Tests"
                | "tests"
                | "test"
                | "fixtures"
                | "testdata"
                | "benches"
                | "examples"
                | "include"
                | "lib"
        ) || is_fixture_path(path)
        {
            under_code_dir = true;
        }
        if matches!(dir.as_ref(), "node_modules" | "__pycache__") {
            return false;
        }
        if !under_code_dir && dir.as_ref() == "target" {
            return false;
        }
        if !under_code_dir
            && matches!(
                dir.as_ref(),
                "dist"
                    | "build"
                    | "results"
                    | "samples"
                    | "artifacts"
                    | "dogfood-output"
                    | "data"
                    | "dataset"
                    | "datasets"
                    | "corpus"
                    | "traces"
                    | "state"
                    | "research"
                    | "benchmarks"
                    | "benchmark"
            )
        {
            return false;
        }
    }
    path.file_name().and_then(|n| n.to_str()) != Some(".DS_Store")
}
