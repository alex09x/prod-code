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

use crate::{DiskSpace, disk_space, seed_fits};

use super::detect::{find_project_types_within, is_typescript_project};
use super::env::{ensure_cache_dir, ts_types_cache_dir};
#[cfg(not(unix))]
use super::merge::merge_types;
use super::merge::{merge_types_within, tree_size, tree_size_within};

/// Seeds TypeScript type declarations and `@types` cache across worktrees.
pub fn seed_typescript_worktree(from: &Path, to: &Path) -> io::Result<Option<u64>> {
    if !is_typescript_project(from) {
        return Ok(None);
    }
    seed_typescript_worktree_within(from, to, disk_space(&ts_types_cache_dir()))
}

fn types_cache_namespace(from: &Path, to: &Path, type_dirs: &[PathBuf]) -> String {
    const MANIFESTS: [&str; 8] = [
        "package.json",
        "package-lock.json",
        "npm-shrinkwrap.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        "deno.lock",
    ];
    let mut key = Vec::new();
    let mut has_lockfile = false;
    for root in [from, to] {
        for name in MANIFESTS {
            if let Ok(contents) = fs::read(root.join(name)) {
                key.extend_from_slice(name.as_bytes());
                key.push(0);
                key.extend_from_slice(&contents);
                key.push(0xff);
                has_lockfile |= name != "package.json";
            }
        }
    }
    for type_dir in type_dirs {
        if type_dir.file_name() != Some(std::ffi::OsStr::new("@types")) {
            let identity = type_dir
                .canonicalize()
                .unwrap_or_else(|_| type_dir.clone())
                .to_string_lossy()
                .into_owned();
            key.extend_from_slice(b"custom-type-root\0");
            key.extend_from_slice(identity.as_bytes());
            key.push(0xff);
        }
    }
    if !has_lockfile {
        // With no resolver lock, keep installations in distinct views rather than guessing that
        // same-named declarations in separate workspaces have identical versions.
        for root in [from, to] {
            let identity = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            key.extend_from_slice(identity.to_string_lossy().as_bytes());
            key.push(0xff);
        }
    }
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&key))
}

/// Seeds TypeScript type declarations respecting a provided disk space budget.
pub fn seed_typescript_worktree_within(
    from: &Path,
    to: &Path,
    space: Option<DiskSpace>,
) -> io::Result<Option<u64>> {
    if !is_typescript_project(from) {
        return Ok(None);
    }

    let mut total_bytes = 0u64;

    // 1. Gather all candidate type declaration directories and isolate each resolver version set.
    let discovered_types = find_project_types_within(from, &[from, to]);
    let cache_dir = ts_types_cache_dir().join(types_cache_namespace(from, to, &discovered_types));
    ensure_cache_dir(&cache_dir)?;
    let cache_dir = cache_dir.canonicalize()?;
    let aggregate_size: u64 = discovered_types
        .iter()
        .map(|d| tree_size_within(d, &[from, to]))
        .sum();

    // 2. Enforce disk budget before merging into shared cache
    if aggregate_size > 0 && seed_fits("typescript types cache", aggregate_size, space) {
        for type_dir in &discovered_types {
            let dir_name = type_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let dst_target = if dir_name == "@types" {
                cache_dir.clone()
            } else {
                cache_dir.join(dir_name)
            };
            let merged = merge_types_within(type_dir, &dst_target, &[from, to])?;
            total_bytes += merged;
        }
    }

    // 3. Establish `to/node_modules/@types` symlink pointing to the shared types cache
    let to_node_modules = to.join("node_modules");
    if let Err(e) = fs::create_dir_all(&to_node_modules) {
        tracing::debug!(error = %e, "creating to/node_modules failed");
    }

    let to_at_types = to_node_modules.join("@types");
    let symlink_meta = fs::symlink_metadata(&to_at_types);
    match symlink_meta {
        Err(_) => {
            #[cfg(unix)]
            {
                if std::os::unix::fs::symlink(&cache_dir, &to_at_types).is_ok() {
                    total_bytes += 1;
                }
            }
            #[cfg(not(unix))]
            {
                let _ = merge_types(&cache_dir, &to_at_types);
            }
        }
        Ok(m) if m.file_type().is_symlink() => {
            // Retarget existing worktrees when their package lock resolves a different type set.
            let points_to_cache =
                fs::canonicalize(&to_at_types).is_ok_and(|target| target == cache_dir);
            if !points_to_cache {
                let _ = fs::remove_file(&to_at_types);
                #[cfg(unix)]
                {
                    if std::os::unix::fs::symlink(&cache_dir, &to_at_types).is_ok() {
                        total_bytes += 1;
                    }
                }
            }
        }
        Ok(m) if m.is_dir() => {
            // If it's a real directory (e.g. copied by dependency trees or created by pnpm),
            // safely dereference valid package links and merge into shared cache before deduplicating.
            let to_size = tree_size_within(&to_at_types, &[from, to]);
            if to_size == 0 || seed_fits("typescript types cache", to_size, space) {
                let merged = merge_types_within(&to_at_types, &cache_dir, &[from, to])?;
                total_bytes += merged;

                // Deduplicate: replace real directory with symlink to shared cache to eliminate duplicate gigabytes
                #[cfg(unix)]
                {
                    let backup = to_node_modules.join(".old-at-types");
                    if fs::rename(&to_at_types, &backup).is_ok() {
                        let to_had_types = tree_size_within(&backup, &[from, to]) > 0;
                        let cache_has_types = tree_size(&cache_dir) > 0;
                        // If backup had valid types, ensure cache has types before committing to symlink
                        if (!to_had_types || cache_has_types)
                            && std::os::unix::fs::symlink(&cache_dir, &to_at_types).is_ok()
                        {
                            let _ = fs::remove_dir_all(&backup);
                        } else {
                            let _ = fs::remove_file(&to_at_types);
                            let _ = fs::rename(&backup, &to_at_types);
                        }
                    }
                }
            }
        }
        _ => {}
    }

    Ok(Some(total_bytes))
}

/// Ensures `typeRoots` in `tsconfig.json` or `jsconfig.json` includes `"node_modules/@types"`
/// if custom `typeRoots` are configured, preventing custom typeRoots from hiding shared types.
pub(crate) fn coordinate_tsconfig(config_path: &Path) {
    if !config_path.is_file() {
        return;
    }
    let Ok(content) = fs::read_to_string(config_path) else {
        return;
    };
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };

    let Some(obj) = val.as_object_mut() else {
        return;
    };

    let compiler_options = obj
        .entry("compilerOptions")
        .or_insert_with(|| serde_json::json!({}));

    if let Some(opts) = compiler_options.as_object_mut() {
        if let Some(type_roots) = opts.get_mut("typeRoots").and_then(|tr| tr.as_array_mut()) {
            let has_node_modules_types = type_roots.iter().any(|v| {
                v.as_str().map_or(false, |s| {
                    s == "node_modules/@types"
                        || s == "./node_modules/@types"
                        || s.ends_with("/node_modules/@types")
                })
            });
            if !has_node_modules_types {
                type_roots.push(serde_json::Value::String("node_modules/@types".to_string()));
                if let Ok(updated) = serde_json::to_string_pretty(&val) {
                    let _ = fs::write(config_path, updated);
                }
            }
        }
    }
}
