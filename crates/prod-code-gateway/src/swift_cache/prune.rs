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
use std::time::{Duration, SystemTime};

use super::env::swift_module_cache_dir;

/// Evicts stale `.pcm` and `.swiftmodule` cache files from `swift_module_cache_dir()`
/// that are older than `max_age`, or when total cache size exceeds `max_size_bytes`
/// (least recently modified files evicted first).
///
/// Returns the number of files removed.
pub fn prune_stale_module_cache(max_age: Duration, max_size_bytes: u64) -> io::Result<usize> {
    prune_stale_module_cache_in(&swift_module_cache_dir(), max_age, max_size_bytes)
}

/// Evicts stale `.pcm` and `.swiftmodule` cache files from `cache_dir`
/// that are older than `max_age`, or when total cache size exceeds `max_size_bytes`
/// (least recently modified files evicted first).
///
/// Uses directory-handle-relative traversal and removal with `O_NOFOLLOW` / `AT_SYMLINK_NOFOLLOW`
/// to eliminate symlink TOCTOU races and guarantee no files outside `cache_dir` can ever be
/// traversed or unlinked (#834).
/// Returns the number of files removed.
pub fn prune_stale_module_cache_in(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
) -> io::Result<usize> {
    if !cache_dir.is_dir() {
        return Ok(0);
    }

    #[cfg(unix)]
    {
        unix_pruner::prune_unix(cache_dir, max_age, max_size_bytes)
    }

    #[cfg(not(unix))]
    {
        prune_fallback(cache_dir, max_age, max_size_bytes)
    }
}

#[cfg(unix)]
mod unix_pruner {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    pub struct CacheEntry {
        pub rel_components: Vec<std::ffi::CString>,
        pub file_name: std::ffi::CString,
        pub size: u64,
        pub modified: SystemTime,
    }

    pub fn prune_unix(
        cache_dir: &Path,
        max_age: Duration,
        max_size_bytes: u64,
    ) -> io::Result<usize> {
        let parent = cache_dir
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let Some(name) = cache_dir.file_name() else {
            return Ok(0);
        };
        let canonical_parent = match fs::canonicalize(parent) {
            Ok(parent) => parent,
            Err(_) => return Ok(0),
        };
        let root_path = canonical_parent.join(name);
        let expected_meta = match fs::symlink_metadata(&root_path) {
            Ok(m) => m,
            Err(_) => return Ok(0),
        };
        if expected_meta.file_type().is_symlink() || !expected_meta.file_type().is_dir() {
            return Ok(0);
        }

        let c_root = std::ffi::CString::new(root_path.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let root_fd = unsafe {
            libc::open(
                c_root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if root_fd < 0 {
            return Ok(0);
        }

        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(root_fd, &mut st) } != 0 {
            unsafe {
                libc::close(root_fd);
            }
            return Ok(0);
        }

        use std::os::unix::fs::MetadataExt;
        if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR
            || (st.st_dev as u64) != expected_meta.dev()
            || (st.st_ino as u64) != expected_meta.ino()
        {
            unsafe {
                libc::close(root_fd);
            }
            return Ok(0);
        }

        let mut files = Vec::new();
        let mut total_size = 0u64;
        let mut rel_components = Vec::new();

        unsafe {
            collect_dir(root_fd, &mut rel_components, &mut files, &mut total_size);
        }

        let now = SystemTime::now();
        let mut removed = 0;

        // 1. Remove files older than max_age
        files.retain(|f| {
            if let Ok(age) = now.duration_since(f.modified) {
                if age > max_age {
                    if remove_entry(root_fd, f) {
                        total_size = total_size.saturating_sub(f.size);
                        removed += 1;
                        return false;
                    }
                }
            }
            true
        });

        // 2. If total size still exceeds max_size_bytes, evict oldest first
        if total_size > max_size_bytes {
            files.sort_by_key(|f| f.modified);
            for f in &files {
                if total_size <= max_size_bytes {
                    break;
                }
                if remove_entry(root_fd, f) {
                    total_size = total_size.saturating_sub(f.size);
                    removed += 1;
                }
            }
        }

        unsafe {
            libc::close(root_fd);
        }

        Ok(removed)
    }

    unsafe fn collect_dir(
        current_fd: libc::c_int,
        rel_components: &mut Vec<std::ffi::CString>,
        files: &mut Vec<CacheEntry>,
        total_size: &mut u64,
    ) {
        unsafe {
            let dup_fd = libc::dup(current_fd);
            if dup_fd < 0 {
                return;
            }
            let dir_stream = libc::fdopendir(dup_fd);
            if dir_stream.is_null() {
                libc::close(dup_fd);
                return;
            }

            loop {
                let entry = libc::readdir(dir_stream);
                if entry.is_null() {
                    break;
                }
                let name_ptr = (*entry).d_name.as_ptr();
                let name = std::ffi::CStr::from_ptr(name_ptr);
                let bytes = name.to_bytes();
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                if bytes.starts_with(b".") || bytes.ends_with(b".lock") {
                    continue;
                }

                let mut st: libc::stat = std::mem::zeroed();
                if libc::fstatat(
                    current_fd,
                    name.as_ptr(),
                    &mut st,
                    libc::AT_SYMLINK_NOFOLLOW,
                ) != 0
                {
                    continue;
                }

                let mode = st.st_mode & libc::S_IFMT;
                if mode == libc::S_IFLNK {
                    continue;
                }
                if mode == libc::S_IFDIR {
                    let child_fd = libc::openat(
                        current_fd,
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    );
                    if child_fd >= 0 {
                        rel_components.push(name.to_owned());
                        collect_dir(child_fd, rel_components, files, total_size);
                        rel_components.pop();
                        libc::close(child_fd);
                    }
                } else if mode == libc::S_IFREG {
                    let modified =
                        SystemTime::UNIX_EPOCH + Duration::from_secs(st.st_mtime.max(0) as u64);
                    let size = st.st_size as u64;
                    *total_size += size;
                    files.push(CacheEntry {
                        rel_components: rel_components.clone(),
                        file_name: name.to_owned(),
                        size,
                        modified,
                    });
                }
            }

            libc::closedir(dir_stream);
        }
    }

    fn remove_entry(root_fd: libc::c_int, entry: &CacheEntry) -> bool {
        let mut current_fd = root_fd;
        let mut fds_to_close = Vec::new();

        for comp in &entry.rel_components {
            let next_fd = unsafe {
                libc::openat(
                    current_fd,
                    comp.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if next_fd < 0 {
                for fd in fds_to_close {
                    unsafe {
                        libc::close(fd);
                    }
                }
                return false;
            }
            fds_to_close.push(next_fd);
            current_fd = next_fd;
        }

        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let is_reg = unsafe {
            libc::fstatat(
                current_fd,
                entry.file_name.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            ) == 0
                && (st.st_mode & libc::S_IFMT) == libc::S_IFREG
        };

        let removed = if is_reg {
            unsafe { libc::unlinkat(current_fd, entry.file_name.as_ptr(), 0) == 0 }
        } else {
            false
        };

        for fd in fds_to_close {
            unsafe {
                libc::close(fd);
            }
        }

        removed
    }
}

#[cfg(not(unix))]
fn prune_fallback(
    _cache_dir: &Path,
    _max_age: Duration,
    _max_size_bytes: u64,
) -> io::Result<usize> {
    Ok(0)
}
