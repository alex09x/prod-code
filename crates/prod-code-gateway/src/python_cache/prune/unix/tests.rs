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
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;

#[test]
fn removal_requires_the_original_inode_and_an_uncontended_target_lock() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("api.pyi");
    fs::write(&path, "original").unwrap();
    let meta = fs::metadata(&path).unwrap();
    let entry = CacheEntry {
        rel_components: vec![],
        file_name: std::ffi::CString::new("api.pyi").unwrap(),
        size: meta.len(),
        modified: SystemTime::UNIX_EPOCH,
        ino: meta.ino(),
        dev: meta.dev(),
    };
    let directory = fs::File::open(temp.path()).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(temp.path().join(".lock-api.pyi"))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert!(!remove_entry(directory.as_raw_fd(), &entry));
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    drop(lock);
    fs::rename(&path, temp.path().join("original.pyi")).unwrap();
    fs::write(&path, "replacement").unwrap();
    assert!(!remove_entry(directory.as_raw_fd(), &entry));
    assert_eq!(fs::read_to_string(&path).unwrap(), "replacement");
    let meta = fs::metadata(&path).unwrap();
    let current = CacheEntry {
        ino: meta.ino(),
        dev: meta.dev(),
        ..entry
    };
    assert!(remove_entry(directory.as_raw_fd(), &current));
    assert!(!path.exists());
    assert!(!remove_entry(directory.as_raw_fd(), &current));
    assert!(!remove_entry(-1, &current));
    let inaccessible = CacheEntry {
        rel_components: vec![std::ffi::CString::new("missing").unwrap()],
        ..current
    };
    assert!(!remove_entry(directory.as_raw_fd(), &inaccessible));
}

#[test]
fn pruning_handles_absent_roots_and_protects_recent_or_locked_temporary_files() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        prune_unix(
            &temp.path().join("missing"),
            Duration::ZERO,
            0,
            Duration::ZERO
        )
        .unwrap(),
        0
    );
    let plain = temp.path().join("file");
    fs::write(&plain, "keep").unwrap();
    assert_eq!(
        prune_unix(&plain, Duration::ZERO, 0, Duration::ZERO).unwrap(),
        0
    );
    let cache = temp.path().join("cache");
    fs::create_dir_all(cache.join("nested")).unwrap();
    let recent = cache.join(".tmp-stub-recent");
    fs::write(&recent, "recent").unwrap();
    let locked = cache.join(".tmp-stub-locked");
    let handle = fs::File::create(&locked).unwrap();
    handle.set_modified(SystemTime::UNIX_EPOCH).unwrap();
    assert_eq!(
        unsafe { libc::flock(handle.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let stale = cache.join("nested/.tmp-stub-abandoned");
    fs::write(&stale, "old").unwrap();
    fs::File::open(&stale)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH)
        .unwrap();
    assert_eq!(
        prune_unix(&cache, Duration::ZERO, 0, Duration::from_secs(60)).unwrap(),
        1
    );
    assert!(!stale.exists());
    assert_eq!(fs::read_to_string(recent).unwrap(), "recent");
    assert!(locked.is_file());
}

/// #1530: an entry still in use is not evicted for its age. Both packages' stubs were published
/// two hours ago; one package has been used since, and only the other goes past a one-hour limit.
#[test]
fn an_entry_in_use_is_not_pruned_for_its_age() {
    let temp = tempfile::tempdir().unwrap();
    let used = temp.path().join("used_pkg/__init__.pyi");
    let idle = temp.path().join("idle_pkg/__init__.pyi");
    let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 3600);
    for path in [&used, &idle] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "def f() -> int: ...\n").unwrap();
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_times(
            fs::FileTimes::new()
                .set_accessed(two_hours_ago)
                .set_modified(two_hours_ago),
        )
        .unwrap();
    }
    crate::python_cache::fs_ops::mark_used(&used);
    let meta = fs::metadata(&used).unwrap();
    assert!(
        meta.modified().unwrap() <= two_hours_ago + Duration::from_secs(1),
        "marking keeps the source's time"
    );
    // A namespace a workspace links to is used as a whole, its nested packages too.
    let nested = temp.path().join("ns-1/pkg/sub/mod.pyi");
    fs::create_dir_all(nested.parent().unwrap()).unwrap();
    fs::write(&nested, "x: int\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&nested)
        .unwrap()
        .set_modified(two_hours_ago)
        .unwrap();
    crate::python_cache::fs_ops::mark_dir_used(&temp.path().join("ns-1"));
    let removed = prune_unix(
        temp.path(),
        Duration::from_secs(3600),
        u64::MAX,
        Duration::from_secs(3600),
    )
    .unwrap();
    assert_eq!(removed, 1);
    assert!(used.exists(), "the used stub stays");
    assert!(nested.exists(), "a stub of a used namespace stays");
    assert!(!idle.exists(), "the idle one goes");
}
