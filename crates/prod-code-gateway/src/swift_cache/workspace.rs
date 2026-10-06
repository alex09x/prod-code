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
use std::io;
use std::path::{Path, PathBuf};

use crate::cpp_index::relocate_path_or_uri;

/// Relocates paths and `file://` URIs in a SwiftPM `workspace-state.json` file.
///
/// In SwiftPM, `.build/workspace-state.json` stores the resolved package dependency graph,
/// checkouts subpaths, local source control repository locations, and binary artifacts.
///
/// Preserves path-component boundaries: sibling paths such as `/work/repo-deps` when relocating
/// `/work/repo` are left completely untouched.
pub fn relocate_swiftpm_workspace_state(
    content: &str,
    from_str: &str,
    to_str: &str,
) -> io::Result<String> {
    if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(content) {
        relocate_json_value(&mut val, from_str, to_str);
        serde_json::to_string_pretty(&val)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    } else {
        // Fallback: line-by-line token relocation if json is not strictly standard
        let mut lines = Vec::new();
        for line in content.lines() {
            lines.push(relocate_path_or_uri(line, from_str, to_str));
        }
        Ok(lines.join("\n"))
    }
}

fn relocate_json_value(val: &mut serde_json::Value, from_str: &str, to_str: &str) {
    match val {
        serde_json::Value::String(s) => {
            let replaced = relocate_path_or_uri(s, from_str, to_str);
            if replaced != *s {
                *s = replaced;
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                relocate_json_value(item, from_str, to_str);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, item) in map {
                relocate_json_value(item, from_str, to_str);
            }
        }
        _ => {}
    }
}

/// Finds all directories containing a `Package.swift` manifest in `root`.
///
/// Returns relative paths from `root`. If `root` itself contains `Package.swift`,
/// `PathBuf::new()` is included. Recursively discovers nested SwiftPM packages
/// (e.g. `clients/macos/ProdUI`), while skipping known cache/node directories.
pub fn find_swift_packages(root: &Path) -> Vec<PathBuf> {
    let mut packages = Vec::new();
    walk_for_packages(root, root, &mut packages);
    packages.sort();
    packages.dedup();
    packages
}

fn walk_for_packages(root: &Path, dir: &Path, packages: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    let mut has_package = false;
    let mut subdirs = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str == "Package.swift" && entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
        {
            has_package = true;
        } else if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            if is_cache_or_ignored_dir(&name_str) {
                continue;
            }
            subdirs.push(entry.path());
        }
    }

    if has_package {
        if let Ok(rel) = dir.strip_prefix(root) {
            packages.push(rel.to_path_buf());
        }
    }

    for subdir in subdirs {
        walk_for_packages(root, &subdir, packages);
    }
}

fn is_cache_or_ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "target"
            | "node_modules"
            | ".venv"
            | "venv"
            | "__pycache__"
            | "build"
            | ".build"
            | ".cache"
            | "DerivedData"
            | ".swiftpm"
            | ".gradle"
    )
}

/// Recursively calculates the total size in bytes of a directory tree.
pub fn tree_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.file_type().is_symlink() {
        return metadata.len();
    }
    if !metadata.is_dir() {
        return metadata.len();
    }
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            total += tree_size(&entry.path());
        }
    }
    total
}

pub(crate) fn real_module_cache_size(path: &Path) -> u64 {
    fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        .map(|_| tree_size(path))
        .unwrap_or(0)
}

pub(crate) fn module_cache_size_in_build(build: &Path) -> u64 {
    let mut total = real_module_cache_size(&build.join("ModuleCache"));
    let Ok(entries) = fs::read_dir(build) else {
        return total;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        for profile in ["debug", "release"] {
            total += real_module_cache_size(&path.join(profile).join("ModuleCache"));
        }
    }
    total
}
