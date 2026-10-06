/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::transport::ScrubSecrets;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::workspace::tree_size;

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(1);

/// Merges non-hidden cache files from `src_dir` into `dst_dir`.
pub(crate) fn merge_cache_files(src_dir: &Path, dst_dir: &Path) -> io::Result<u64> {
    let mut copied = 0u64;
    let Ok(entries) = fs::read_dir(src_dir) else {
        return Ok(0);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let target = dst_dir.join(&name);
        let target_metadata = fs::symlink_metadata(&target).ok();
        if target_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            continue;
        }
        if metadata.is_file() && target_metadata.is_none() {
            copied += fs::copy(&path, &target)?;
        } else if metadata.is_dir() {
            fs::create_dir_all(&target)?;
            copied += merge_cache_files(&path, &target)?;
        }
    }
    Ok(copied)
}

/// Copies a directory tree preserving modification times and symbolic links.
/// Uses a unique per-attempt staging holder directory (created with exclusive mkdir)
/// and atomic rename to ensure concurrent seeding attempts never clobber, nest into,
/// or delete each other's destinations (#834).
pub(crate) fn copy_dir_preserving(src: &Path, dst: &Path) -> io::Result<u64> {
    if dst.exists() {
        return Ok(tree_size(dst));
    }

    let parent = dst
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"))?;
    fs::create_dir_all(parent)?;

    let dst_name = dst.file_name().and_then(|n| n.to_str()).unwrap_or("dir");

    // Exclusively create a unique staging holder directory.
    // If the directory already exists (e.g. from an earlier crashed process),
    // loop and retry with a new unique nonce.
    let mut attempts = 0;
    let (holder, staging_dst) = loop {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            ^ ((NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed) as u128) << 64)
            ^ ((std::process::id() as u128) << 32);
        let holder_path = parent.join(format!(".staging-holder-{dst_name}-{nonce:032x}"));
        match fs::create_dir(&holder_path) {
            Ok(()) => {
                let staging_path = holder_path.join("content");
                break (holder_path, staging_path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                attempts += 1;
                if attempts > 1000 {
                    return Err(io::Error::other(
                        "exhausted attempts to create exclusive staging directory",
                    ));
                }
            }
            Err(e) => return Err(e),
        }
    };

    #[cfg(unix)]
    let copy_result = {
        let mut cmd = std::process::Command::new("cp");
        cmd.scrub_cluster_secrets();
        let status = cmd.arg("-a").arg(src).arg(&staging_dst).status();
        match status {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(io::Error::other(format!(
                "copying {} to {} failed: {s}",
                src.display(),
                staging_dst.display()
            ))),
            Err(e) => Err(e),
        }
    };

    #[cfg(not(unix))]
    let copy_result = copy_dir_fallback(src, &staging_dst).map(|_| ());

    if let Err(e) = copy_result {
        let _ = fs::remove_dir_all(&holder);
        return Err(e);
    }

    // Try atomic rename from staging_dst to dst (same filesystem, since holder is in parent)
    if let Err(e) = fs::rename(&staging_dst, dst) {
        // If dst was already populated concurrently, clean up holder and accept dst
        let _ = fs::remove_dir_all(&holder);
        if dst.exists() {
            return Ok(tree_size(dst));
        }
        return Err(e);
    }

    // Clean up empty holder directory
    let _ = fs::remove_dir(&holder);

    Ok(tree_size(dst))
}

#[cfg(not(unix))]
pub(crate) fn copy_dir_fallback(src: &Path, dst: &Path) -> io::Result<u64> {
    let mut total = 0u64;
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            total += copy_dir_fallback(&from, &to)?;
        } else if from.is_file() {
            total += fs::copy(&from, &to)?;
        }
    }
    Ok(total)
}
