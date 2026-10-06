/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;



/// The parts of a cargo target directory that a new worktree's copy takes from the main copy
/// when it is seeded: compiled crates, build-script outputs and cargo's fingerprints. A registry
/// crate has the same source path in every copy, so its fingerprint still matches and it is not
/// compiled again; only the workspace's own crates are (#278: 8 crates in 37.0 s against 305 in
/// 106.2 s). Incremental caches belong to those crates and are left behind. The copy is the
/// worktree's own: nothing is shared afterwards, so no build waits on another's lock.
pub(crate) const SEEDED_BUILD_DIRS: &[&str] = &["deps", "build", ".fingerprint"];

/// Free and total bytes of a filesystem.
#[derive(Debug, Clone, Copy)]
pub struct DiskSpace {
    pub free: u64,
    pub total: u64,
}

/// The share of its filesystem a seed must leave free (#419): above the janitor's 15% prune
/// line (#386), so seeding a worktree never pushes the node into pruning or into the disk
/// pressure that placement avoids (#396).
pub(crate) const SEED_MIN_FREE_SHARE: f64 = 0.20;

/// Whether copying `size` bytes of `what` fits in `space`: twice the size free, and a fifth of
/// the filesystem still free afterwards. A copy that does not fit is logged as skipped: the
/// worktree's first build or install is then slower, not broken (#419).
pub fn seed_fits(what: &str, size: u64, space: Option<DiskSpace>) -> bool {
    let Some(space) = space else {
        return false;
    };
    let after = space.free.saturating_sub(size);
    let fits = space.free >= size.saturating_mul(2)
        && after as f64 >= SEED_MIN_FREE_SHARE * space.total as f64;
    if !fits {
        let mb = |bytes: u64| bytes / (1024 * 1024);
        tracing::info!(
            what,
            size_mb = mb(size),
            free_mb = mb(space.free),
            total_mb = mb(space.total),
            "🌱 [SEED] skipped: the copy would leave too little disk free"
        );
    }
    fits
}

/// Copies the seed copy's `target/debug` build cache into the new copy at `to`, when there is
/// one and it fits (`seed_fits`). Returns the bytes copied, or `None` when there was nothing to
/// copy or no room for it.
pub fn seed_build_cache(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<Option<u64>> {
    seed_build_cache_within(from, to, disk_space(to))
}

pub(crate) fn seed_build_cache_within(
    from: &std::path::Path,
    to: &std::path::Path,
    space: Option<DiskSpace>,
) -> std::io::Result<Option<u64>> {
    let source = from.join("target").join("debug");
    let parts: Vec<&str> = SEEDED_BUILD_DIRS
        .iter()
        .copied()
        .filter(|part| source.join(part).is_dir())
        .collect();
    if parts.is_empty() {
        return Ok(None);
    }
    let size: u64 = parts.iter().map(|part| tree_size(&source.join(part))).sum();
    if !seed_fits("target/debug", size, space) {
        return Ok(None);
    }
    let dest = to.join("target").join("debug");
    std::fs::create_dir_all(&dest)?;
    for part in parts {
        // `cp -a` keeps modification times: cargo compares a crate's outputs with those of the
        // crates it depends on, and fresh times in copy order would make half of them stale.
        let status = std::process::Command::new("cp")
            .arg("-a")
            .arg(source.join(part))
            .arg(&dest)
            .status()?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "copying {} failed: {status}",
                source.join(part).display()
            )));
        }
    }
    Ok(Some(size))
}


/// Bytes of every regular file under `dir`.
pub fn tree_size(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => tree_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// Free and total bytes of the filesystem that holds `path` (or its nearest existing parent).
pub fn disk_space(path: &std::path::Path) -> Option<DiskSpace> {
    use std::os::unix::ffi::OsStrExt;
    let existing = path.ancestors().find(|p| p.exists())?;
    let c_path = std::ffi::CString::new(existing.as_os_str().as_bytes()).ok()?;
    // SAFETY: an all-zero `statvfs` is a valid value for the call to fill in.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` is a valid NUL-terminated path and `stat` is valid for writes.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    // The fields are `u64` on Linux and narrower on macOS.
    #[allow(clippy::unnecessary_cast)]
    let (free, total) = (
        stat.f_bavail as u64 * stat.f_frsize as u64,
        stat.f_blocks as u64 * stat.f_frsize as u64,
    );
    Some(DiskSpace { free, total })
}

/// Copies the sources of the seed copy `src` into the new copy `dst`: every per-node cache
/// (`is_node_cache`) stays behind. Some of them hold the seed copy's absolute paths. A CMake
/// `build/` made the new worktree's `check` fail on the old `CMakeCache.txt`, and its
/// `compile_commands.json` pointed clangd at the other copy's sources (#416). The caches that are
/// safe to move are seeded on purpose: `seed_build_cache` and `seed_dependency_trees`.
pub fn copy_tree(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<usize> {
    let mut copied = 0;
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if is_node_cache(&name.to_string_lossy()) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if entry.file_type()?.is_symlink() && from.is_dir() {
            // A directory symlink stays one: walking it copied its target a second time, or
            // without end when it points at a parent (#414).
            std::os::unix::fs::symlink(std::fs::read_link(&from)?, &to)?;
        } else if from.is_dir() {
            // A virtual environment is copied whole, its symlinks kept, by
            // `seed_dependency_trees`.
            if !is_virtualenv(&from) {
                copied += copy_tree(&from, &to)?;
            }
        } else if from.is_file() {
            std::fs::copy(&from, &to)?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// Build products, dependency trees and virtual environments: per-node caches, not sources. They
/// never travel back to the client, and they are not the client's to delete.
pub fn is_node_cache(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "target"
            | "node_modules"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
            | ".tox"
            | ".nox"
            | "build"
            | ".build"
            | "dist"
            | ".cache"
            | ".next"
            | ".turbo"
            | "coverage"
            | "DerivedData"
            | ".swiftpm"
            | ".gradle"
    ) || name == workspace::LAST_USED_MARKER
        || name == workspace::STALE_MARKER
}

pub fn walk_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if is_node_cache(&name) {
            continue;
        }
        if name == "typings" && path.is_symlink() {
            let cache_root = python_cache::python_stub_cache_dir();
            if python_cache::is_shared_stub_cache_link(&path, &cache_root) {
                continue;
            }
        }
        if path.is_dir() {
            walk_files(root, &path, out);
        } else if path.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, path));
        }
    }
}

