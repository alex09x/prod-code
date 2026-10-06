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
use std::sync::atomic::{AtomicU64, Ordering};

use prod_code_protocol::transport::ScrubSecrets;

use crate::{DiskSpace, disk_space, seed_fits};

use super::detect::{find_venv_stubs, is_python_project};
use super::env::{ensure_cache_dir, python_stub_cache_dir};
use super::fingerprint::python_stub_cache_namespace;
use super::merge::{merge_stubs, tree_size};

#[allow(dead_code)]
static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(1);

pub fn seed_python_worktree(from: &Path, to: &Path) -> io::Result<Option<u64>> {
    if !is_python_project(from) {
        return Ok(None);
    }
    seed_python_worktree_within(from, to, disk_space(&python_stub_cache_dir()))
}

/// Seeds Python type stubs respecting a provided disk space budget.
pub fn seed_python_worktree_within(
    from: &Path,
    to: &Path,
    space: Option<DiskSpace>,
) -> io::Result<Option<u64>> {
    if !is_python_project(from) {
        return Ok(None);
    }

    let mut total_bytes = 0u64;

    // 1. Gather candidate stubs and isolate cache views by the resolved Python dependency set.
    let from_typings = from.join("typings");
    let to_typings = to.join("typings");
    let mut from_venv_stubs = Vec::new();
    let mut to_venv_stubs = Vec::new();
    for venv_name in &[".venv", "venv"] {
        let from_venv = from.join(venv_name);
        if from_venv.is_dir() {
            from_venv_stubs.extend(find_venv_stubs(&from_venv));
        }
        let to_venv = to.join(venv_name);
        if to_venv.is_dir() {
            to_venv_stubs.extend(find_venv_stubs(&to_venv));
        }
    }
    let (namespace, compatible) = python_stub_cache_namespace(
        from,
        to,
        &from_venv_stubs,
        &to_venv_stubs,
        &from_typings,
        &to_typings,
    );
    let cache_dir = python_stub_cache_dir().join(namespace);
    ensure_cache_dir(&cache_dir)?;
    let venv_stubs = if compatible {
        from_venv_stubs
    } else {
        to_venv_stubs
    };
    let to_has_real_typings = fs::symlink_metadata(&to_typings)
        .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink());
    let source_typings = if compatible {
        from_typings.is_dir().then_some(&from_typings)
    } else {
        to_has_real_typings.then_some(&to_typings)
    };
    let typings_size = source_typings.map_or(0, |dir| tree_size(dir));
    let venv_stubs_size: u64 = venv_stubs.iter().map(|s| tree_size(s)).sum();
    let aggregate_stubs_size = typings_size + venv_stubs_size;

    // Enforce aggregate disk budget across both project typings and virtual environment stubs
    if aggregate_stubs_size > 0 && seed_fits("python type stubs cache", aggregate_stubs_size, space)
    {
        if let Some(source_typings) = source_typings {
            if typings_size > 0 {
                let merged = merge_stubs(source_typings, &cache_dir)?;
                total_bytes += merged;
            }
        }

        for stub_dir in venv_stubs {
            let stub_name = stub_dir.file_name().unwrap_or_default();
            let dst_stub = cache_dir.join(stub_name);
            let merged = merge_stubs(&stub_dir, &dst_stub)?;
            total_bytes += merged;
        }
    }

    // 3. Establish `to/typings` symlink pointing to this dependency-version cache view.
    match fs::symlink_metadata(&to_typings) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let expected = cache_dir
                .canonicalize()
                .unwrap_or_else(|_| cache_dir.clone());
            if fs::canonicalize(&to_typings).ok().as_deref() != Some(expected.as_path()) {
                fs::remove_file(&to_typings)?;
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(&expected, &to_typings)?;
                    total_bytes += 1;
                }
                #[cfg(not(unix))]
                {
                    let _ = copy_dir_preserving(&cache_dir, &to_typings);
                }
            }
        }
        Err(_) => {
            if let Some(parent) = to_typings.parent() {
                fs::create_dir_all(parent)?;
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&cache_dir, &to_typings)?;
                total_bytes += 1;
            }
            #[cfg(not(unix))]
            {
                let _ = copy_dir_preserving(&cache_dir, &to_typings);
            }
        }
        Ok(meta) if meta.is_dir() => {
            // Keep a worktree's real typings directory available while merging its declarations.
            let merged = merge_stubs(&to_typings, &cache_dir)?;
            total_bytes += merged;
        }
        _ => {}
    }

    // 4. Update pyrightconfig.json in `to` if present to include stubPath
    let to_pyright_config = to.join("pyrightconfig.json");
    if to_pyright_config.is_file() {
        if let Ok(content) = fs::read_to_string(&to_pyright_config) {
            if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(obj) = val.as_object_mut() {
                    if !obj.contains_key("stubPath") {
                        obj.insert(
                            "stubPath".to_string(),
                            serde_json::Value::String("typings".to_string()),
                        );
                        if let Ok(updated) = serde_json::to_string_pretty(&val) {
                            let _ = fs::write(&to_pyright_config, updated);
                        }
                    }
                }
            }
        }
    }

    Ok(Some(total_bytes))
}

#[allow(dead_code)]
fn copy_dir_preserving(src: &Path, dst: &Path) -> io::Result<u64> {
    if dst.exists() {
        return Ok(tree_size(dst));
    }
    let parent = dst
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"))?;
    fs::create_dir_all(parent)?;

    let dst_name = dst.file_name().and_then(|n| n.to_str()).unwrap_or("stubs");

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
        cmd.arg("-a").arg(src).arg(&staging_dst);
        cmd.scrub_cluster_secrets();
        match cmd.status() {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(io::Error::other(format!(
                "cp -a {} {} failed with status {}",
                src.display(),
                staging_dst.display(),
                s
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

    if let Err(e) = fs::rename(&staging_dst, dst) {
        let _ = fs::remove_dir_all(&holder);
        if dst.exists() {
            return Ok(tree_size(dst));
        }
        return Err(e);
    }

    let _ = fs::remove_dir(&holder);
    Ok(tree_size(dst))
}

#[cfg(not(unix))]
fn copy_dir_fallback(src: &Path, dst: &Path) -> io::Result<u64> {
    fs::create_dir_all(dst)?;
    let mut total = 0;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ft.is_dir() {
            total += copy_dir_fallback(&entry.path(), &target)?;
        } else if ft.is_file() {
            total += fs::copy(&entry.path(), &target)?;
        }
    }
    Ok(total)
}
