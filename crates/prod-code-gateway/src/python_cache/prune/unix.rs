/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#![cfg(unix)]

use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::timestamp::parse_tmp_stub_timestamp;

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
    tmp_grace_period: Duration,
) -> io::Result<usize> {
    let canonical_root = match fs::canonicalize(cache_dir) {
        Ok(c) => c,
        Err(_) => return Ok(0),
    };
    let expected_meta = match fs::symlink_metadata(&canonical_root) {
        Ok(m) => m,
        Err(_) => return Ok(0),
    };
    if !expected_meta.file_type().is_dir() {
        return Ok(0);
    }

    let c_root = std::ffi::CString::new(canonical_root.as_os_str().as_bytes())
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

    let now = SystemTime::now();
    let mut removed = 0;

    unsafe {
        collect_dir(
            root_fd,
            &mut rel_components,
            &mut files,
            &mut total_size,
            now,
            tmp_grace_period,
            &mut removed,
        );
    }

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
    now: SystemTime,
    tmp_grace_period: Duration,
    removed: &mut usize,
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
            if bytes.ends_with(b".lock") {
                continue;
            }
            let is_tmp_stub = bytes.starts_with(b".tmp-stub-");
            if bytes.starts_with(b".") && !is_tmp_stub {
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

            if is_tmp_stub {
                if mode == libc::S_IFREG {
                    let modified =
                        SystemTime::UNIX_EPOCH + Duration::from_secs(st.st_mtime.max(0) as u64);
                    let file_size = st.st_size as u64;
                    let name_str = name.to_str().unwrap_or("");
                    let created_at = parse_tmp_stub_timestamp(name_str);
                    let age = match created_at {
                        Some(ts) => now.duration_since(ts).unwrap_or_else(|_| {
                            now.duration_since(modified).unwrap_or(Duration::ZERO)
                        }),
                        None => now.duration_since(modified).unwrap_or(Duration::ZERO),
                    };

                    // Check whether an active publisher holds an OS lock/lease on this temporary file
                    let is_locked = is_temp_file_locked(current_fd, name.as_ptr());

                    if !is_locked && age > tmp_grace_period {
                        // Abandoned temporary file: unlink immediately
                        if libc::unlinkat(current_fd, name.as_ptr(), 0) == 0 {
                            *removed += 1;
                        }
                    } else {
                        // Active publisher holding lock or recent temporary file: account for disk space in cache budget,
                        // but DO NOT add to eviction candidate list `files` to protect active publishers.
                        *total_size += file_size;
                    }
                }
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
                    collect_dir(
                        child_fd,
                        rel_components,
                        files,
                        total_size,
                        now,
                        tmp_grace_period,
                        removed,
                    );
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

unsafe fn is_temp_file_locked(dir_fd: libc::c_int, name: *const libc::c_char) -> bool {
    unsafe {
        let fd = libc::openat(
            dir_fd,
            name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        );
        if fd < 0 {
            // Cannot open descriptor (e.g. concurrency or permissions) - err on side of caution
            return true;
        }

        let ret = libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB);
        if ret == 0 {
            // Successfully acquired lock: no other process holds an exclusive lock
            libc::flock(fd, libc::LOCK_UN);
            libc::close(fd);
            false
        } else {
            // Lock attempt failed: an active publisher holds an exclusive lock on this file!
            libc::close(fd);
            true
        }
    }
}
