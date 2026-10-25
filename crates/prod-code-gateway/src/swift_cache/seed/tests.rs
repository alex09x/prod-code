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

#[cfg(unix)]
#[test]
fn linking_merges_local_products_and_replaces_only_outdated_cache_links() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let build = temp.path().join("target");
    let shared = temp.path().join("shared");
    fs::create_dir_all(source.join("ModuleCache")).unwrap();
    fs::create_dir_all(build.join("ModuleCache")).unwrap();
    fs::create_dir(&shared).unwrap();
    fs::write(source.join("ModuleCache/source.pcm"), "source").unwrap();
    fs::write(build.join("ModuleCache/target.pcm"), "target").unwrap();
    link_shared_module_cache(&source, &build.join("ModuleCache"), &shared).unwrap();
    assert_eq!(fs::read_link(build.join("ModuleCache")).unwrap(), shared);
    assert_eq!(
        fs::read_to_string(shared.join("source.pcm")).unwrap(),
        "source"
    );
    assert_eq!(
        fs::read_to_string(shared.join("target.pcm")).unwrap(),
        "target"
    );
    link_shared_module_cache(&source, &build.join("ModuleCache"), &shared).unwrap();
    fs::remove_file(build.join("ModuleCache")).unwrap();
    symlink(temp.path().join("missing"), build.join("ModuleCache")).unwrap();
    link_shared_module_cache(&source, &build.join("ModuleCache"), &shared).unwrap();
    assert_eq!(fs::read_link(build.join("ModuleCache")).unwrap(), shared);
    for profile in ["debug", "release"] {
        let cache = source
            .join("sample-triple")
            .join(profile)
            .join("ModuleCache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join(format!("{profile}.pcm")), profile).unwrap();
    }
    fs::write(source.join("not-a-directory"), "skip").unwrap();
    fs::create_dir(source.join(".hidden")).unwrap();
    link_triple_module_caches(&source, &build, &shared).unwrap();
    for profile in ["debug", "release"] {
        assert_eq!(
            fs::read_link(
                build
                    .join("sample-triple")
                    .join(profile)
                    .join("ModuleCache")
            )
            .unwrap(),
            shared
        );
        assert_eq!(
            fs::read_to_string(shared.join(format!("{profile}.pcm"))).unwrap(),
            profile
        );
    }
    link_triple_module_caches(&temp.path().join("missing"), &build, &shared).unwrap();
    assert!(!build.join("not-a-directory").exists());
}

#[cfg(unix)]
#[test]
fn seeding_rejects_non_utf8_roots_before_creating_a_target() {
    use std::os::unix::ffi::OsStringExt;
    let temp = tempfile::tempdir().unwrap();
    let invalid = temp.path().join(std::ffi::OsString::from_vec(vec![0xff]));
    // Some filesystems (UTF-8-only ZFS datasets, APFS) refuse such a name outright; the seed's
    // own check is then unreachable from a real path.
    if fs::create_dir(&invalid).is_err() {
        return;
    }
    fs::write(invalid.join("Package.swift"), "// package").unwrap();
    let target = temp.path().join("target");
    assert_eq!(
        seed_swift_worktree_with_budgets(&invalid, &target, None, None)
            .unwrap_err()
            .to_string(),
        "from is not valid UTF-8"
    );
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("Package.swift"), "// package").unwrap();
    assert_eq!(
        seed_swift_worktree_with_budgets(&source, &invalid, None, None)
            .unwrap_err()
            .to_string(),
        "to is not valid UTF-8"
    );
    assert!(!target.exists());
}
