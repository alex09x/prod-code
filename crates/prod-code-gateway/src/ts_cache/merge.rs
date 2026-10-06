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

use super::env::ensure_cache_dir;
use super::fs_ops::copy_and_publish_type_file;
use super::roots::{
    VisitedDirs, approved_target, build_approved_roots, find_enclosing_project_root,
};

/// Recursively copies and merges type declarations from `src_dir` into `dst_dir`.
///
/// Only declaration files (`.d.ts`, `.d.mts`, `.d.cts`, `.d.ts.map`, `.json`, etc.)
/// and subdirectories containing them are indexed.
/// Safely dereferences valid package symlinks (such as pnpm package symlinks into virtual stores)
/// while strictly rejecting out-of-root symlinks and tracking visited directory inodes to prevent cycles (#835, #836).
/// Returns total bytes written or updated.
pub fn merge_types(src_dir: &Path, dst_dir: &Path) -> io::Result<u64> {
    let project_root = find_enclosing_project_root(src_dir);
    merge_types_within(src_dir, dst_dir, &[&project_root])
}

/// Recursively copies and merges type declarations from `src_dir` into `dst_dir` constraining
/// symlinks to explicit approved roots (such as `from` and `to` project worktrees).
pub fn merge_types_within(
    src_dir: &Path,
    dst_dir: &Path,
    approved_roots: &[&Path],
) -> io::Result<u64> {
    let approved = build_approved_roots(approved_roots);
    let mut visited = VisitedDirs::default();
    merge_types_inner(src_dir, dst_dir, &approved, &mut visited)
}

fn is_type_declaration_file(name: &str) -> bool {
    name.ends_with(".d.ts")
        || name.ends_with(".d.mts")
        || name.ends_with(".d.cts")
        || name.ends_with(".d.ts.map")
        || name.ends_with(".d.mts.map")
        || name.ends_with(".d.cts.map")
        || name == "package.json"
        || name == "tsconfig.json"
        || name.ends_with(".json")
}

fn merge_types_inner(
    src_dir: &Path,
    dst_dir: &Path,
    approved_roots: &[PathBuf],
    visited: &mut VisitedDirs,
) -> io::Result<u64> {
    let Some(canonical_src) = approved_target(src_dir, approved_roots) else {
        tracing::debug!(src_dir = %src_dir.display(), "skipping traversal root outside approved roots");
        return Ok(0);
    };
    if !canonical_src.is_dir() || !visited.insert(&canonical_src) {
        return Ok(0);
    }
    ensure_cache_dir(dst_dir)?;

    let mut bytes_written = 0u64;
    let Ok(entries) = fs::read_dir(&canonical_src) else {
        return Ok(0);
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        if name_str.starts_with('.') {
            continue;
        }

        // Safely dereference symlinks (e.g. pnpm package symlinks into virtual stores).
        // Resolves the canonical target once, verifies it is inside approved roots, and uses
        // that validated canonical target for all subsequent metadata, traversal, and copy operations
        // to prevent TOCTOU symlink swaps (#836).
        let (effective_path, is_dir, is_file) = if file_type.is_symlink() {
            let Some(canon) = approved_target(&path, approved_roots) else {
                tracing::debug!(path = %path.display(), "skipping symlink pointing outside approved roots");
                continue;
            };
            match fs::metadata(&canon) {
                Ok(meta) => (canon, meta.is_dir(), meta.is_file()),
                Err(_) => continue, // dangling symlink, skip safely
            }
        } else {
            (path, file_type.is_dir(), file_type.is_file())
        };

        if is_dir {
            let sub_dst = dst_dir.join(&file_name);
            let sub_bytes = merge_types_inner(&effective_path, &sub_dst, approved_roots, visited)?;
            bytes_written += sub_bytes;
        } else if is_file && is_type_declaration_file(&name_str) {
            let target_file = dst_dir.join(&file_name);
            let written = copy_and_publish_type_file(&effective_path, &target_file)?;
            bytes_written += written;
        }
    }

    Ok(bytes_written)
}

/// Recursively computes total size of all regular declaration files in a directory,
/// safely dereferencing valid symlinks within approved roots and tracking visited inodes to prevent cycles.
pub fn tree_size(dir: &Path) -> u64 {
    let project_root = find_enclosing_project_root(dir);
    tree_size_within(dir, &[&project_root])
}

/// Computes total size of declaration files in a directory, constraining symlinks to explicit approved roots.
pub fn tree_size_within(dir: &Path, approved_roots: &[&Path]) -> u64 {
    let approved = build_approved_roots(approved_roots);
    let mut visited = VisitedDirs::default();
    tree_size_inner(dir, &approved, &mut visited)
}

fn tree_size_inner(dir: &Path, approved_roots: &[PathBuf], visited: &mut VisitedDirs) -> u64 {
    let Some(canonical_dir) = approved_target(dir, approved_roots) else {
        return 0;
    };
    if !canonical_dir.is_dir() || !visited.insert(&canonical_dir) {
        return 0;
    }
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(&canonical_dir) {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };

            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            if name_str.starts_with('.') {
                continue;
            }

            // Safely dereference symlinks for sizing using validated canonical target (#836).
            let (effective_path, is_dir, is_file, file_len) = if file_type.is_symlink() {
                let Some(canon) = approved_target(&path, approved_roots) else {
                    continue;
                };
                match fs::metadata(&canon) {
                    Ok(meta) => {
                        let len = if meta.is_file() { meta.len() } else { 0 };
                        (canon, meta.is_dir(), meta.is_file(), len)
                    }
                    Err(_) => continue,
                }
            } else {
                let len = if file_type.is_file() {
                    entry.metadata().map(|m| m.len()).unwrap_or(0)
                } else {
                    0
                };
                (path, file_type.is_dir(), file_type.is_file(), len)
            };

            if is_dir {
                total += tree_size_inner(&effective_path, approved_roots, visited);
            } else if is_file && is_type_declaration_file(&name_str) {
                total += file_len;
            }
        }
    }
    total
}
