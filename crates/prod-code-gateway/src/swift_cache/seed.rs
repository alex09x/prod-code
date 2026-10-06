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
use std::path::Path;

use super::env::swift_module_cache_dir;
use super::fs_ops::{copy_dir_preserving, merge_cache_files};
use super::workspace::{
    find_swift_packages, module_cache_size_in_build, relocate_swiftpm_workspace_state, tree_size,
};

/// Seeds SwiftPM package checkouts, bare repositories, binary artifacts, workspace state,
/// and links `.build/ModuleCache` to the node's shared Swift module cache directory (Roadmap 3.7).
///
/// Returns `Ok(Some(bytes_seeded))` if any Swift packages were found and initialized,
/// or `Ok(None)` if no Swift packages exist in `from`.
pub fn seed_swift_worktree(from: &Path, to: &Path) -> io::Result<Option<u64>> {
    seed_swift_worktree_within(from, to, crate::disk_space(to))
}

/// Seeds SwiftPM package checkouts, bare repositories, binary artifacts, workspace state,
/// and links `.build/ModuleCache` to the node's shared Swift module cache directory (Roadmap 3.7),
/// respecting the filesystem disk space budget `space`.
pub fn seed_swift_worktree_within(
    from: &Path,
    to: &Path,
    space: Option<crate::DiskSpace>,
) -> io::Result<Option<u64>> {
    let packages = find_swift_packages(from);
    if packages.is_empty() {
        return Ok(None);
    }

    let from_str = from
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "from is not valid UTF-8"))?;
    let to_str = to
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "to is not valid UTF-8"))?;

    let shared_module_cache = swift_module_cache_dir();
    let mut total_bytes = 0u64;
    let mut any_seeded = false;

    let heavy_names = &["checkouts", "repositories", "artifacts"];
    let mut heavy_size = 0u64;

    for rel in &packages {
        let from_pkg = from.join(rel);
        let to_pkg = to.join(rel);

        let from_build = from_pkg.join(".build");
        let to_build = to_pkg.join(".build");

        if from_build.is_dir() {
            for &sub_name in heavy_names {
                let from_sub = from_build.join(sub_name);
                let to_sub = to_build.join(sub_name);
                if from_sub.is_dir() && !to_sub.exists() {
                    heavy_size += tree_size(&from_sub);
                }
            }
        }
    }

    let module_cache_size: u64 = packages
        .iter()
        .map(|rel| {
            module_cache_size_in_build(&from.join(rel).join(".build"))
                + module_cache_size_in_build(&to.join(rel).join(".build"))
        })
        .sum();
    let total_seed_size = heavy_size.saturating_add(module_cache_size);
    let cache_seed_fits = total_seed_size == 0
        || crate::seed_fits(
            "SwiftPM dependency and module caches",
            total_seed_size,
            space,
        );
    let shared_cache_fits = module_cache_size == 0
        || crate::seed_fits(
            "shared Swift module cache",
            module_cache_size,
            crate::disk_space(&shared_module_cache),
        );
    let heavy_fits = cache_seed_fits;
    let module_cache_fits = cache_seed_fits && shared_cache_fits;

    for rel in packages {
        let from_pkg = from.join(&rel);
        let to_pkg = to.join(&rel);

        let from_build = from_pkg.join(".build");
        let to_build = to_pkg.join(".build");

        // Ensure target .build directory exists
        fs::create_dir_all(&to_build)?;

        // 1. Link module caches only when the complete source/target cache set fits the budget.
        if module_cache_fits {
            let to_module_cache = to_build.join("ModuleCache");
            link_shared_module_cache(&from_build, &to_module_cache, &shared_module_cache)?;
            any_seeded = true;

            // Also link triple-specific caches such as debug/ModuleCache.
            link_triple_module_caches(&from_build, &to_build, &shared_module_cache)?;
        } else {
            tracing::info!(
                workspace = %to_pkg.display(),
                cache_bytes = module_cache_size,
                "Swift module cache exceeds disk budget; leaving local module caches in place"
            );
        }

        // 3. Seed package checkouts, bare repositories, and binary artifacts if from_build exists and fits
        if from_build.is_dir() {
            if heavy_fits {
                for &sub_name in heavy_names {
                    let from_sub = from_build.join(sub_name);
                    let to_sub = to_build.join(sub_name);

                    if from_sub.is_dir() && !to_sub.exists() {
                        let sub_bytes = copy_dir_preserving(&from_sub, &to_sub)?;
                        total_bytes += sub_bytes;
                        any_seeded = true;
                    }
                }
            }

            // 4. Relocate and seed workspace-state.json
            let from_state = from_build.join("workspace-state.json");
            let to_state = to_build.join("workspace-state.json");
            if from_state.is_file() {
                if let Ok(state_content) = fs::read_to_string(&from_state) {
                    if let Ok(relocated_state) =
                        relocate_swiftpm_workspace_state(&state_content, from_str, to_str)
                    {
                        fs::write(&to_state, relocated_state.as_bytes())?;
                        total_bytes += relocated_state.len() as u64;
                        any_seeded = true;
                    }
                }
            }
        }

        // 5. Seed .swiftpm configuration if present
        let from_swiftpm = from_pkg.join(".swiftpm");
        let to_swiftpm = to_pkg.join(".swiftpm");
        if from_swiftpm.is_dir() && !to_swiftpm.exists() {
            let bytes = copy_dir_preserving(&from_swiftpm, &to_swiftpm)?;
            total_bytes += bytes;
            any_seeded = true;
        }
    }

    if any_seeded {
        Ok(Some(total_bytes))
    } else {
        Ok(None)
    }
}

/// Links `to_module_cache` to `shared_module_cache`.
///
/// If `from_build/ModuleCache` exists and is a regular directory (not a symlink to shared cache),
/// copies any precompiled `.pcm` or `.swiftmodule` cache files into `shared_module_cache`
/// before establishing the link, preserving existing warm compilation products.
fn link_shared_module_cache(
    from_build: &Path,
    to_module_cache: &Path,
    shared_module_cache: &Path,
) -> io::Result<()> {
    let from_module_cache = from_build.join("ModuleCache");
    if from_module_cache.is_dir() {
        // If from has a real directory with cached modules, merge them into shared_module_cache
        if let Ok(meta) = fs::symlink_metadata(&from_module_cache) {
            if !meta.file_type().is_symlink() {
                merge_cache_files(&from_module_cache, shared_module_cache)?;
            }
        }
    }

    // If to_module_cache is already a symlink pointing to shared_module_cache, keep it
    if let Ok(meta) = fs::symlink_metadata(to_module_cache) {
        if meta.file_type().is_symlink() {
            if let Ok(target) = fs::read_link(to_module_cache) {
                if target == shared_module_cache {
                    return Ok(());
                }
            }
            let _ = fs::remove_file(to_module_cache);
        } else if meta.is_dir() {
            merge_cache_files(to_module_cache, shared_module_cache)?;
            let _ = fs::remove_dir_all(to_module_cache);
        }
    }

    // Create symlink
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(shared_module_cache, to_module_cache)?;
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(shared_module_cache, to_module_cache)?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        fs::create_dir_all(to_module_cache)?;
    }

    Ok(())
}

/// Discovers any triple-specific ModuleCache directories (e.g. `<triple>/debug/ModuleCache`)
/// and establishes symlinks to `shared_module_cache`.
fn link_triple_module_caches(
    from_build: &Path,
    to_build: &Path,
    shared_module_cache: &Path,
) -> io::Result<()> {
    if !from_build.is_dir() {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(from_build) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.')
            || matches!(
                name_str.as_ref(),
                "checkouts" | "repositories" | "artifacts" | "ModuleCache"
            )
        {
            continue;
        }
        let from_triple = entry.path();
        if !from_triple.is_dir() {
            continue;
        }
        for profile in &["debug", "release"] {
            let from_profile = from_triple.join(profile);
            let from_cache = from_profile.join("ModuleCache");
            if from_cache.is_dir() {
                let to_profile = to_build.join(name.as_os_str()).join(profile);
                fs::create_dir_all(&to_profile)?;
                let to_cache = to_profile.join("ModuleCache");
                let _ = link_shared_module_cache(&from_profile, &to_cache, shared_module_cache);
            }
        }
    }
    Ok(())
}
