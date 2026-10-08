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
use std::path::Path;
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

#[test]
fn test_mypypath_view_exposes_stub_only_packages_by_import_name() {
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    let requests_stubs = cache_dir.join("requests-stubs");
    fs::create_dir_all(&requests_stubs).unwrap();
    fs::write(
        requests_stubs.join("__init__.pyi"),
        "def get(url: str): ...\n",
    )
    .unwrap();

    let mypy_dir = ensure_mypypath_view(&cache_dir).unwrap();
    assert_eq!(mypy_dir, cache_dir.join(".mypypath"));

    // Verify requests-stubs is exposed under the requests import name for Mypy
    let requests_import = mypy_dir.join("requests");
    assert!(
        requests_import.exists(),
        "requests import package must exist in .mypypath"
    );
    assert!(
        requests_import.join("__init__.pyi").is_file(),
        "requests/__init__.pyi must be accessible through the mypy view"
    );

    let envs = python_stub_cache_env_for_dir(&cache_dir);
    let mypy_val = envs
        .iter()
        .find(|(k, _)| k == "MYPYPATH")
        .map(|(_, v)| v.as_str());
    assert_eq!(mypy_val, Some(mypy_dir.to_str().unwrap()));

    let typings_val = envs
        .iter()
        .find(|(k, _)| k == "TYPINGS_PATH")
        .map(|(_, v)| v.as_str());
    assert_eq!(typings_val, Some(cache_dir.to_str().unwrap()));

    assert!(
        cache_dir
            .join("requests-stubs")
            .join("__init__.pyi")
            .is_file()
    );
}

#[test]
fn test_seed_python_worktree_exposes_mypypath_view() {
    let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("shared-stubs-mypypath");
    unsafe {
        std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom_cache);
    }

    let from = temp.path().join("origin");
    let to = temp.path().join("worktree");
    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("setup.py"), "# setup\n").unwrap();

    let sp = from
        .join(".venv")
        .join("lib")
        .join("python3.11")
        .join("site-packages");
    let req_stubs = sp.join("requests-stubs");
    fs::create_dir_all(&req_stubs).unwrap();
    fs::write(req_stubs.join("__init__.pyi"), "def get(url: str): ...\n").unwrap();

    seed_python_worktree(&from, &to).unwrap();

    let envs = python_stub_cache_env_for_workspace(&to);
    let mypypath = envs
        .iter()
        .find(|(k, _)| k == "MYPYPATH")
        .map(|(_, v)| v.as_str())
        .expect("MYPYPATH must be present");

    let mypy_dir = Path::new(mypypath);
    assert!(
        mypy_dir.join("requests").join("__init__.pyi").is_file(),
        "requests import package must exist under MYPYPATH"
    );

    // Verify workspace typings has the PEP 561 view
    let to_typings = to.join("typings");
    assert!(
        to_typings
            .join("requests-stubs")
            .join("__init__.pyi")
            .is_file(),
        "typings must preserve PEP 561 requests-stubs for Pyright"
    );

    unsafe {
        std::env::remove_var(PYTHON_STUB_CACHE_ENV);
    }
}

#[test]
fn test_mypypath_view_copy_fallback_populates_packages() {
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    let req_stubs = cache_dir.join("requests-stubs");
    let sub = req_stubs.join("adapters");
    fs::create_dir_all(&sub).unwrap();
    fs::write(req_stubs.join("__init__.pyi"), "def get(): ...\n").unwrap();
    fs::write(sub.join("mod.pyi"), "class HTTPAdapter: ...\n").unwrap();
    fs::write(cache_dir.join("top_level.pyi"), "# top level\n").unwrap();

    let mypy_dir = cache_dir.join(".mypypath_copy");
    fs::create_dir_all(&mypy_dir).unwrap();

    let mut map = std::collections::BTreeMap::new();
    map.insert("requests".to_string(), "requests-stubs".to_string());
    let direct_files = vec!["top_level.pyi".to_string()];

    env::populate_mypypath_view_copy(&cache_dir, &mypy_dir, &map, &direct_files).unwrap();

    let req = mypy_dir.join("requests");
    assert!(req.join("__init__.pyi").is_file());
    assert!(req.join("adapters").join("mod.pyi").is_file());
    assert!(mypy_dir.join("top_level.pyi").is_file());
}

#[test]
fn test_mypypath_view_fallback_on_unready_directory() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let mypy = cache.join(".mypypath");
    fs::create_dir_all(&mypy).unwrap();
    fs::write(mypy.join("partial.pyi"), "# incomplete").unwrap();

    let envs = python_stub_cache_env_for_dir(&cache);
    let val = envs.iter().find(|(k, _)| k == "MYPYPATH");
    assert_eq!(val.map(|(_, v)| v.as_str()), Some(cache.to_str().unwrap()));
}

#[test]
fn test_seed_python_worktree_propagates_mypypath_error() {
    let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    unsafe {
        std::env::set_var(PYTHON_STUB_CACHE_ENV, &cache);
    }

    let (from, to) = (temp.path().join("from"), temp.path().join("to"));
    let p_stubs = from.join(".venv/lib/python3.11/site-packages/p-stubs");
    fs::create_dir_all(&p_stubs).unwrap();
    fs::write(from.join("setup.py"), "#").unwrap();
    fs::write(p_stubs.join("__init__.pyi"), "x: int\n").unwrap();

    let (ns, _) = fingerprint::python_stub_cache_namespace(
        &from,
        &to,
        &[p_stubs],
        &[],
        &from.join("typings"),
        &to.join("typings"),
    );
    let target = cache.join(ns);
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join(".mypypath"), "blocker").unwrap();

    assert!(seed_python_worktree(&from, &to).is_err());
    unsafe {
        std::env::remove_var(PYTHON_STUB_CACHE_ENV);
    }
}
