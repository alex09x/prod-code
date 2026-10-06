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
#[cfg(not(unix))]
use std::path::PathBuf;
use std::time::SystemTime;

use super::env::ensure_cache_dir;
use super::fs_ops::copy_and_publish_stub;

#[derive(Default)]
struct VisitedDirs {
    #[cfg(unix)]
    dev_ino: std::collections::HashSet<(u64, u64)>,
    #[cfg(not(unix))]
    canonical: std::collections::HashSet<PathBuf>,
}

impl VisitedDirs {
    fn insert(&mut self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(meta) = fs::symlink_metadata(path) {
                if meta.file_type().is_symlink() {
                    return false;
                }
                return self.dev_ino.insert((meta.dev(), meta.ino()));
            }
            false
        }
        #[cfg(not(unix))]
        {
            if let Ok(meta) = fs::symlink_metadata(path) {
                if meta.file_type().is_symlink() {
                    return false;
                }
            }
            if let Ok(canon) = path.canonicalize() {
                return self.canonical.insert(canon);
            }
            false
        }
    }
}

/// Recursively copies/merges type stubs from `src_dir` into `dst_dir`.
///
/// Only `.pyi`, `.typed`, and package directories containing them are indexed.
/// Strictly skips directory symlinks and tracks visited directory inodes to prevent recursion cycles (#835).
/// Returns total bytes written or updated.
pub fn merge_stubs(src_dir: &Path, dst_dir: &Path) -> io::Result<u64> {
    let mut visited = VisitedDirs::default();
    merge_stubs_inner(src_dir, dst_dir, &mut visited)
}

fn merge_stubs_inner(src_dir: &Path, dst_dir: &Path, visited: &mut VisitedDirs) -> io::Result<u64> {
    if !src_dir.is_dir() || !visited.insert(src_dir) {
        return Ok(0);
    }
    ensure_cache_dir(dst_dir)?;

    let mut bytes_written = 0u64;
    let Ok(entries) = fs::read_dir(src_dir) else {
        return Ok(0);
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        // Strictly skip symlinks to prevent traversal outside cache or cyclic recursion
        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        if name_str.starts_with('.') {
            continue;
        }

        let target_path = dst_dir.join(&file_name);

        if file_type.is_dir() {
            bytes_written += merge_stubs_inner(&path, &target_path, visited)?;
        } else if file_type.is_file() {
            let is_stub_file =
                name_str.ends_with(".pyi") || name_str == "py.typed" || name_str.ends_with(".py");

            if !is_stub_file {
                continue;
            }

            let should_check = match (fs::metadata(&path), fs::metadata(&target_path)) {
                (Ok(src_meta), Ok(dst_meta)) => {
                    let src_mod = src_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    let dst_mod = dst_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    src_mod > dst_mod || (src_mod == dst_mod && src_meta.len() != dst_meta.len())
                }
                (Ok(_), Err(_)) => true,
                _ => false,
            };

            if should_check {
                let copied = copy_and_publish_stub(&path, &target_path)?;
                bytes_written += copied;
            }
        }
    }

    Ok(bytes_written)
}

pub fn tree_size(dir: &Path) -> u64 {
    let mut visited = VisitedDirs::default();
    tree_size_inner(dir, &mut visited)
}

fn tree_size_inner(dir: &Path, visited: &mut VisitedDirs) -> u64 {
    if !visited.insert(dir) {
        return 0;
    }
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                total += tree_size_inner(&entry.path(), visited);
            } else if file_type.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            }
        }
    }
    total
}
