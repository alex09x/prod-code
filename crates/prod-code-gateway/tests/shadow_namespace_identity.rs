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

use prod_code_gateway::shadow::{OWNERSHIP_LOCK_FILE, ShadowRootOwner, default_root};
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::Path;
use tempfile::TempDir;

fn canonical(path: &Path) -> std::path::PathBuf {
    path.canonicalize()
        .unwrap_or_else(|error| panic!("cannot canonicalize {}: {error}", path.display()))
}

#[test]
fn nonexistent_storage_aliases_share_one_owned_namespace() {
    let fixture = TempDir::new().expect("create fixture");
    let storage = fixture.path().join("storage");
    let alias = fixture.path().join("missing").join("..").join("storage");
    let sentinel = fixture.path().join("live-sentinel");
    fs::write(&sentinel, "unchanged").expect("write sentinel");

    let before = default_root(&alias);
    assert!(!storage.exists(), "default_root must not create storage");
    let first = ShadowRootOwner::acquire(&before).expect("acquire before storage exists");

    fs::create_dir_all(&storage).expect("create storage");
    let after = default_root(&storage);
    assert_eq!(
        canonical(&before),
        after,
        "aliases must choose one namespace"
    );
    assert_eq!(
        fs::metadata(canonical(&before).join(OWNERSHIP_LOCK_FILE))
            .expect("read first ownership lock")
            .ino(),
        fs::metadata(after.join(OWNERSHIP_LOCK_FILE))
            .expect("read second ownership lock")
            .ino(),
        "aliases must use the same ownership-lock inode"
    );
    assert!(
        ShadowRootOwner::acquire(&after).is_err(),
        "the second alias must contend on the existing ownership lock"
    );
    assert_eq!(fs::read(&sentinel).expect("read sentinel"), b"unchanged");

    drop(first);
    ShadowRootOwner::acquire(&after).expect("release must allow another owner");
}

#[test]
fn symlink_prefix_and_unresolved_parent_traversal_remain_stable() {
    let fixture = TempDir::new().expect("create fixture");
    let target = fixture.path().join("target");
    fs::create_dir(&target).expect("create symlink target");
    let link = fixture.path().join("storage-link");
    symlink(&target, &link).expect("create prefix symlink");
    let storage = target.join("storage");
    let alias = link.join("absent").join("..").join("storage");

    let before = default_root(&alias);
    assert!(!storage.exists(), "default_root must not create storage");
    let owner = ShadowRootOwner::acquire(&before).expect("acquire symlinked namespace");
    fs::create_dir_all(&storage).expect("create storage");
    assert_eq!(canonical(&before), default_root(&storage));
    drop(owner);
}

#[test]
fn unresolved_prefixes_resume_at_existing_symlinked_directories() {
    let fixture = TempDir::new().expect("create fixture");
    let target = fixture.path().join("target");
    fs::create_dir(&target).expect("create symlink target");
    let link = fixture.path().join("storage-link");
    symlink(&target, &link).expect("create prefix symlink");
    let storage = target.join("storage");
    let alias = fixture
        .path()
        .join("missing")
        .join("nested")
        .join("..")
        .join("..")
        .join("storage-link")
        .join("absent")
        .join("nested")
        .join("..")
        .join("..")
        .join("storage");
    let sentinel = fixture.path().join("live-sentinel");
    fs::write(&sentinel, "unchanged").expect("write sentinel");

    let before = default_root(&alias);
    assert!(!storage.exists(), "default_root must not create storage");
    let first = ShadowRootOwner::acquire(&before).expect("acquire before storage exists");

    fs::create_dir_all(&storage).expect("create storage");
    let after = default_root(&storage);
    assert_eq!(
        canonical(&before),
        after,
        "aliases must choose one namespace"
    );
    assert_eq!(
        fs::metadata(canonical(&before).join(OWNERSHIP_LOCK_FILE))
            .expect("read first ownership lock")
            .ino(),
        fs::metadata(after.join(OWNERSHIP_LOCK_FILE))
            .expect("read second ownership lock")
            .ino(),
        "aliases must use the same ownership-lock inode"
    );
    assert!(
        ShadowRootOwner::acquire(&after).is_err(),
        "the second alias must contend on the existing ownership lock"
    );
    assert_eq!(fs::read(&sentinel).expect("read sentinel"), b"unchanged");

    drop(first);
    ShadowRootOwner::acquire(&after).expect("release must allow another owner");
}

#[test]
fn relative_and_absolute_spellings_converge_without_chdir() {
    let cwd = std::env::current_dir().expect("read current directory");
    let fixture = tempfile::Builder::new()
        .prefix("shadow-namespace-")
        .tempdir_in(&cwd)
        .expect("create fixture below current directory");
    let relative = fixture
        .path()
        .strip_prefix(&cwd)
        .expect("fixture must be below current directory");
    let relative_alias = relative.join("nested").join("..").join("storage");
    let absolute = fixture.path().join("storage");

    assert_eq!(default_root(&relative_alias), default_root(&absolute));
    assert!(!absolute.exists(), "default_root must not create storage");
}

#[test]
fn existing_simple_paths_keep_distinct_stable_namespaces() {
    let fixture = TempDir::new().expect("create fixture");
    let first = fixture.path().join("first");
    let second = fixture.path().join("second");
    fs::create_dir_all(&first).expect("create first storage");
    fs::create_dir_all(&second).expect("create second storage");

    assert_eq!(default_root(&first), default_root(&canonical(&first)));
    assert_ne!(default_root(&first), default_root(&second));
}
