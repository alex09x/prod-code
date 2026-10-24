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

#[test]
fn directory_copy_publishes_once_and_removes_staging_after_failure() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested/module.pcm"), "module").unwrap();
    let destination = temp.path().join("destination");
    assert_eq!(copy_dir_preserving(&source, &destination).unwrap(), 6);
    fs::write(source.join("nested/module.pcm"), "replacement").unwrap();
    assert_eq!(copy_dir_preserving(&source, &destination).unwrap(), 6);
    assert_eq!(
        fs::read_to_string(destination.join("nested/module.pcm")).unwrap(),
        "module"
    );
    assert_eq!(
        copy_dir_preserving(&temp.path().join("missing"), &temp.path().join("failed"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::Other
    );
    assert!(!temp.path().join("failed").exists());
    assert!(!fs::read_dir(temp.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".staging")
    }));
}

#[cfg(unix)]
#[test]
fn merge_preserves_existing_files_and_refuses_source_and_target_symlinks() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir(&target).unwrap();
    for (name, content) in [
        ("new.pcm", "new"),
        ("keep.pcm", "replacement"),
        (".hidden", "hidden"),
        ("nested/child.swiftmodule", "child"),
        ("blocked.pcm", "blocked"),
    ] {
        fs::write(source.join(name), content).unwrap();
    }
    fs::write(target.join("keep.pcm"), "keep").unwrap();
    let outside = temp.path().join("outside");
    fs::write(&outside, "outside").unwrap();
    symlink(&outside, source.join("link.pcm")).unwrap();
    symlink(&outside, target.join("blocked.pcm")).unwrap();
    assert_eq!(merge_cache_files(&source, &target).unwrap(), 8);
    assert_eq!(fs::read_to_string(target.join("new.pcm")).unwrap(), "new");
    assert_eq!(
        fs::read_to_string(target.join("nested/child.swiftmodule")).unwrap(),
        "child"
    );
    assert_eq!(fs::read_to_string(target.join("keep.pcm")).unwrap(), "keep");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");
    assert!(!target.join(".hidden").exists());
    assert!(!target.join("link.pcm").exists());
    assert_eq!(
        merge_cache_files(&temp.path().join("missing"), &target).unwrap(),
        0
    );
}
