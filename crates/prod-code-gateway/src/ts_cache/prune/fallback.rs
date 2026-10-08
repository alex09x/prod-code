/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#![cfg(not(unix))]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::timestamp::parse_tmp_ts_timestamp;

pub fn prune_fallback(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
    tmp_grace_period: Duration,
) -> io::Result<usize> {
    if !cache_dir.is_dir() {
        return Ok(0);
    }
    let now = SystemTime::now();
    let mut removed = 0;
    let mut files = Vec::new();
    let mut total_size = 0u64;

    fn walk(
        dir: &Path,
        files: &mut Vec<(PathBuf, u64, SystemTime)>,
        total_size: &mut u64,
        now: SystemTime,
        tmp_grace_period: Duration,
        removed: &mut usize,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".lock") {
                continue;
            }
            let is_tmp_ts = name_str.starts_with(".tmp-ts-");
            if name_str.starts_with('.') && !is_tmp_ts {
                continue;
            }

            if file_type.is_dir() {
                walk(&path, files, total_size, now, tmp_grace_period, removed);
            } else if file_type.is_file() {
                let Ok(meta) = fs::metadata(&path) else {
                    continue;
                };
                let modified_time = meta.modified().ok();
                let modified = modified_time.unwrap_or(SystemTime::UNIX_EPOCH);
                let retention_time =
                    super::non_unix_retention_time(meta.created().ok(), modified_time);
                let size = meta.len();
                let created_at = parse_tmp_ts_timestamp(&name_str);
                let age = match created_at {
                    Some(ts) => now
                        .duration_since(ts)
                        .unwrap_or_else(|_| now.duration_since(modified).unwrap_or(Duration::ZERO)),
                    None => now.duration_since(modified).unwrap_or(Duration::ZERO),
                };

                if is_tmp_ts {
                    let is_locked = fs::OpenOptions::new().write(true).open(&path).is_err();

                    if !is_locked && age > tmp_grace_period {
                        if fs::remove_file(&path).is_ok() {
                            *removed += 1;
                        }
                    } else {
                        *total_size += size;
                    }
                } else {
                    *total_size += size;
                    files.push((path, size, retention_time));
                }
            }
        }
    }

    walk(
        cache_dir,
        &mut files,
        &mut total_size,
        now,
        tmp_grace_period,
        &mut removed,
    );

    // Evict files older than max_age
    files.retain(|(path, size, modified)| {
        if let Ok(age) = now.duration_since(*modified) {
            if age > max_age {
                if fs::remove_file(path).is_ok() {
                    *total_size = total_size.saturating_sub(*size);
                    removed += 1;
                    return false;
                }
            }
        }
        true
    });

    // Evict oldest if still over budget
    if total_size > max_size_bytes {
        files.sort_by_key(|(_, _, modified)| *modified);
        for (path, size, _) in files {
            if total_size <= max_size_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total_size = total_size.saturating_sub(size);
                removed += 1;
            }
        }
    }

    Ok(removed)
}
