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
use crate::sync::include::{SyncInclude, is_non_overridable_sync_path};
use crate::sync::relevance::{is_code_source_or_manifest_file, is_relevant_code_or_manifest_file};
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
    let path = Path::new(rel_path);
    for component in path.parent().into_iter().flat_map(Path::components) {
        if let std::path::Component::Normal(dir) = component
            && matches!(dir.to_str(), Some(".git" | ".prod"))
        {
            return false;
        }
    }
    if is_relevant_code_or_manifest_file(rel_path) {
        return true;
    }
    let mut under_code_dir = false;
    let mut under_pkg_dir = false;
    for component in path.parent().into_iter().flat_map(Path::components) {
        let std::path::Component::Normal(dir) = component else {
            continue;
        };
        let dir = dir.to_string_lossy();
        if matches!(dir.as_ref(), "crates" | "packages") {
            under_pkg_dir = true;
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
                | "benchmarks"
                | "benchmark"
                | "bench"
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
        if !under_code_dir && !under_pkg_dir && is_output_dir(&dir) {
            return false;
        }
        // Source code git tracks is a package, wherever it sits: a vendored Go package under
        // `…/envoy/data/accesslog/v3` is what `-mod=vendor` builds (#2049).
        if !under_code_dir
            && !under_pkg_dir
            && is_data_dir(&dir)
            && !is_code_source_or_manifest_file(path)
        {
            return false;
        }
    }
    path.file_name().and_then(|n| n.to_str()) != Some(".DS_Store")
}

fn is_output_dir(dir: &str) -> bool {
    matches!(
        dir,
        "dist" | "build" | "results" | "samples" | "artifacts" | "dogfood-output"
    )
}

fn is_data_dir(dir: &str) -> bool {
    matches!(
        dir,
        "data" | "dataset" | "datasets" | "corpus" | "traces" | "state" | "research"
    )
}

/// Whether a file of a data directory (`docs/data/backends/alias.yaml`) sits beside source code
/// in its own directory (`embed.go`, which embeds `*.yaml`): that directory is a package, and
/// its files are the package's inputs (#2048). Output directories stay out.
fn is_beside_code(root: &Path, include: &SyncInclude, rel_path: &str) -> bool {
    let path = Path::new(rel_path);
    let dirs: Vec<String> = path
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| match component {
            std::path::Component::Normal(dir) => Some(dir.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if !dirs.iter().any(|dir| is_data_dir(dir))
        || dirs
            .iter()
            .any(|dir| is_output_dir(dir) || matches!(dir.as_str(), "node_modules" | "target"))
        || is_non_overridable_sync_path(path)
    {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    // Read once per directory: a scan asks for each of its files.
    include.package_dir(parent, || {
        std::fs::read_dir(root.join(parent)).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                is_package_source(&entry.path()) && entry.file_type().is_ok_and(|k| k.is_file())
            })
        })
    })
}

/// A file that makes its directory a package: source code, not a manifest or a settings file
/// (`settings.toml`, `deps.lock` and `*.json` sit beside plain data too).
fn is_package_source(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| !matches!(ext, "json" | "toml" | "lock" | "sum" | "work"))
        && is_code_source_or_manifest_file(path)
}

/// [`is_synced_git_path`], plus explicit `[sync] include` paths from `prod-code.toml` (#1836),
/// plus the files of a data directory that holds source code (#2048).
pub fn is_synced_git_path_with(rel_path: &str, include: &SyncInclude) -> bool {
    is_synced_git_path(rel_path)
        || include.is_included(rel_path)
        || include
            .root()
            .is_some_and(|root| is_beside_code(root, include, rel_path))
}

#[cfg(test)]
#[path = "data_package_tests.rs"]
mod data_package_tests;
