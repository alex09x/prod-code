/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use crate::DiskSpace;
use std::fs;
use std::time::{Duration, SystemTime};

static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
#[test]
fn shared_stub_cache_link_is_recognized_by_canonical_target() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache").join("namespace");
    fs::create_dir_all(&cache).unwrap();
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let link = workspace.join("typings");
    symlink(&cache, &link).unwrap();

    assert!(is_shared_stub_cache_link(
        &link,
        temp.path().join("cache").as_path()
    ));
}

#[test]
fn test_python_stub_cache_env_and_path() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("my-custom-python-stubs");

    unsafe {
        std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom_cache);
    }
    let dir = python_stub_cache_dir();
    assert_eq!(dir, custom_cache);
    assert!(dir.is_dir());

    let envs = python_stub_cache_env();
    let custom_str = custom_cache.to_str().unwrap();
    assert!(
        envs.iter()
            .any(|(k, v)| k == PYTHON_STUB_CACHE_ENV && v == custom_str)
    );
    assert!(envs.iter().any(|(k, v)| k == "MYPYPATH" && v == custom_str));
    assert!(
        envs.iter()
            .any(|(k, v)| k == "TYPINGS_PATH" && v == custom_str)
    );

    unsafe {
        std::env::remove_var(PYTHON_STUB_CACHE_ENV);
    }
}

#[test]
fn test_is_python_project_detection() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    assert!(!is_python_project(root));

    fs::write(root.join("pyproject.toml"), "[project]\nname=\"foo\"\n").unwrap();
    assert!(is_python_project(root));
}

#[test]
fn test_merge_stubs_and_seed_worktree() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("shared-stubs");
    unsafe {
        std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom_cache);
    }

    let from = temp.path().join("origin-project");
    let to = temp.path().join("worktree-project");
    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("pyproject.toml"), "[project]\nname = \"demo\"\n").unwrap();

    // Add local typings
    let from_typings = from.join("typings").join("requests");
    fs::create_dir_all(&from_typings).unwrap();
    fs::write(
        from_typings.join("__init__.pyi"),
        "def get(url: str): ...\n",
    )
    .unwrap();

    // Run seed
    // Keep the test independent of the remote node's current filesystem pressure.
    let result = seed_python_worktree_within(
        &from,
        &to,
        Some(DiskSpace {
            free: 1_000_000,
            total: 1_000_000,
        }),
    )
    .unwrap();
    assert!(
        result.is_some_and(|bytes| bytes > 0),
        "Python stub seeding did not publish any bytes"
    );

    // Verify stubs were merged into this dependency-set's cache view.
    let to_typings = to.join("typings");
    assert!(
        fs::symlink_metadata(&to_typings)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let cache_view = fs::read_link(&to_typings).unwrap();
    assert!(cache_view.starts_with(&custom_cache));
    assert!(cache_view.join("requests").join("__init__.pyi").is_file());

    unsafe {
        std::env::remove_var(PYTHON_STUB_CACHE_ENV);
    }
}

#[test]
fn test_parse_tmp_stub_timestamp() {
    let now = SystemTime::now();
    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!(".tmp-stub-1234-5-{nanos:x}");
    let parsed = parse_tmp_stub_timestamp(&name).unwrap();
    let diff = if now > parsed {
        now.duration_since(parsed).unwrap()
    } else {
        parsed.duration_since(now).unwrap()
    };
    assert!(diff < Duration::from_millis(1));

    assert!(parse_tmp_stub_timestamp("regular.pyi").is_none());
    assert!(parse_tmp_stub_timestamp(".tmp-stub-invalid").is_none());
    assert!(parse_tmp_stub_timestamp(".tmp-stub-active-worker-2").is_none());
    assert!(parse_tmp_stub_timestamp(".tmp-stub-1-1-ffffffffffffffffffffffffffffffff").is_none());
}

#[cfg(unix)]
#[test]
fn test_remove_entry_rejects_mismatched_inode() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    let temp = tempfile::tempdir().unwrap();
    let file_path = temp.path().join("stub.pyi");
    fs::write(&file_path, b"content").unwrap();

    let meta = fs::metadata(&file_path).unwrap();
    let old_ino = meta.ino();
    let dev = meta.dev();

    let c_root = std::ffi::CString::new(temp.path().as_os_str().as_bytes()).unwrap();
    let root_fd = unsafe { libc::open(c_root.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
    assert!(root_fd >= 0);

    let entry = prune::unix::CacheEntry {
        rel_components: Vec::new(),
        file_name: std::ffi::CString::new("stub.pyi").unwrap(),
        size: 7,
        modified: SystemTime::now(),
        ino: old_ino + 1, // Intentional mismatched inode!
        dev,
    };

    let removed = prune::unix::remove_entry(root_fd, &entry);
    assert!(!removed, "entry with mismatched inode must not be removed");
    assert!(file_path.is_file(), "file must still exist on disk");

    unsafe {
        libc::close(root_fd);
    }
}

#[cfg(unix)]
#[test]
fn test_remove_entry_removes_matching_inode() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    let temp = tempfile::tempdir().unwrap();
    let file_path = temp.path().join("stub.pyi");
    fs::write(&file_path, b"content").unwrap();

    let meta = fs::metadata(&file_path).unwrap();
    let ino = meta.ino();
    let dev = meta.dev();

    let c_root = std::ffi::CString::new(temp.path().as_os_str().as_bytes()).unwrap();
    let root_fd = unsafe { libc::open(c_root.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
    assert!(root_fd >= 0);

    let entry = prune::unix::CacheEntry {
        rel_components: Vec::new(),
        file_name: std::ffi::CString::new("stub.pyi").unwrap(),
        size: 7,
        modified: SystemTime::now(),
        ino,
        dev,
    };

    let removed = prune::unix::remove_entry(root_fd, &entry);
    assert!(removed, "entry with matching inode must be removed");
    assert!(!file_path.exists(), "file must have been unlinked");

    unsafe {
        libc::close(root_fd);
    }
}
