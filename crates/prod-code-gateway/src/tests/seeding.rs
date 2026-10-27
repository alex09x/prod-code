/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::*;
use std::path::PathBuf;

/// A new worktree's copy takes the seed's compiled crates, build-script outputs and
/// fingerprints with their modification times, and not its incremental caches (#278).
#[test]
fn a_seeded_copy_takes_the_build_cache_with_its_times() {
    let dir = tempfile::tempdir().unwrap();
    let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
    let debug = seed.join("target/debug");
    for (rel, text) in [
        ("deps/libdep-1a.rlib", "rlib"),
        ("build/dep-2b/out/generated.rs", "pub const X: u8 = 1;"),
        (".fingerprint/dep-1a/lib-dep", "fingerprint"),
        ("incremental/shop-3c/s-1/query-cache.bin", "incremental"),
    ] {
        let path = debug.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    std::fs::File::options()
        .write(true)
        .open(debug.join("deps/libdep-1a.rlib"))
        .unwrap()
        .set_modified(old)
        .unwrap();

    let copied = seed_build_cache_within(&seed, &fresh, roomy()).unwrap();
    assert_eq!(copied, Some(4 + 20 + 11));
    let out = fresh.join("target/debug");
    assert_eq!(
        std::fs::read_to_string(out.join("build/dep-2b/out/generated.rs")).unwrap(),
        "pub const X: u8 = 1;"
    );
    assert!(out.join(".fingerprint/dep-1a/lib-dep").is_file());
    assert!(!out.join("incremental").exists());
    let modified = std::fs::metadata(out.join("deps/libdep-1a.rlib"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        modified, old,
        "cargo compares these times; the copy must keep them"
    );
}

/// A new worktree's copy takes the seed's `node_modules` trees, the root's and a workspace
/// package's, with their symlinks kept as symlinks, and none from under `target` or `.git`;
/// without room for two of them it takes none (#412).
#[test]
fn a_seeded_copy_takes_the_node_modules_trees() {
    let dir = tempfile::tempdir().unwrap();
    let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
    for (rel, text) in [
        (
            "node_modules/zod/index.d.ts",
            "export declare const z: unknown;",
        ),
        ("node_modules/typescript/bin/tsc", "#!/usr/bin/env node"),
        ("node_modules/zod/node_modules/inner/index.js", "nested"),
        ("packages/app/node_modules/left-pad/index.js", "pad"),
        ("target/node_modules/stray.js", "not a package tree"),
        ("src/index.ts", "import { z } from 'zod';"),
    ] {
        let path = seed.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    std::fs::create_dir_all(seed.join("node_modules/.bin")).unwrap();
    std::os::unix::fs::symlink("../typescript/bin/tsc", seed.join("node_modules/.bin/tsc"))
        .unwrap();

    assert_eq!(
        dependency_trees(&seed),
        vec![
            PathBuf::from("node_modules"),
            PathBuf::from("packages/app/node_modules")
        ]
    );
    assert_eq!(
        seed_dependency_trees_within(&seed, &fresh, space(10, 1 << 40)).unwrap(),
        None,
        "no room for two of them"
    );
    assert!(!fresh.join("node_modules").exists());

    let copied = seed_dependency_trees_within(&seed, &fresh, roomy()).unwrap();
    assert_eq!(copied, Some(32 + 19 + 6 + 3));
    assert_eq!(
        std::fs::read_to_string(fresh.join("node_modules/zod/index.d.ts")).unwrap(),
        "export declare const z: unknown;"
    );
    assert!(
        fresh
            .join("node_modules/zod/node_modules/inner/index.js")
            .is_file()
    );
    assert!(
        fresh
            .join("packages/app/node_modules/left-pad/index.js")
            .is_file()
    );
    let link = fresh.join("node_modules/.bin/tsc");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        PathBuf::from("../typescript/bin/tsc")
    );
    assert!(!fresh.join("target").exists());
    assert!(!fresh.join("src").exists(), "sources are copy_tree's");

    let bare = dir.path().join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    assert_eq!(
        seed_dependency_trees_within(&bare, &dir.path().join("fresh2"), roomy()).unwrap(),
        None
    );
}

/// A virtual environment reaches the new copy with its symlinks kept (`lib64 -> lib` is not a
/// second copy of site-packages) and its scripts naming the copy; `copy_tree` leaves it to
/// the seeding of dependency trees, and keeps a directory symlink as a symlink instead of
/// walking it (#414).
#[test]
fn a_seeded_copy_takes_a_virtualenv_with_its_links_and_its_paths_rewritten() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
    let (venv, copy) = (seed.join(".venv"), fresh.join(".venv"));
    let old = venv.to_str().unwrap().to_string();
    for (rel, text) in [
        ("pyvenv.cfg", "home = /usr/bin\n".to_string()),
        (
            "lib/python3.12/site-packages/pkg/__init__.py",
            "x = 1\n".to_string(),
        ),
        ("bin/pytest", format!("#!{old}/bin/python\nimport pytest\n")),
        (
            "bin/activate",
            format!("VIRTUAL_ENV='{old}'\nexport VIRTUAL_ENV\n"),
        ),
    ] {
        let path = venv.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    std::fs::set_permissions(
        venv.join("bin/pytest"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::os::unix::fs::symlink("lib", venv.join("lib64")).unwrap();
    std::os::unix::fs::symlink("/usr/bin/python3", venv.join("bin/python")).unwrap();
    std::fs::create_dir_all(seed.join("src")).unwrap();
    std::fs::write(seed.join("src/app.py"), "import pkg\n").unwrap();
    std::os::unix::fs::symlink(".", seed.join("src/again")).unwrap();

    assert_eq!(copy_tree(&seed, &fresh).unwrap(), 1, "only src/app.py");
    assert!(!copy.exists(), "the venv is not copy_tree's");
    let again = std::fs::symlink_metadata(fresh.join("src/again")).unwrap();
    assert!(again.file_type().is_symlink());

    assert_eq!(dependency_trees(&seed), vec![PathBuf::from(".venv")]);
    assert!(
        seed_dependency_trees_within(&seed, &fresh, roomy())
            .unwrap()
            .is_some()
    );
    let link = |rel: &str| std::fs::read_link(copy.join(rel)).unwrap();
    assert_eq!(link("lib64"), PathBuf::from("lib"));
    assert_eq!(link("bin/python"), PathBuf::from("/usr/bin/python3"));
    assert!(
        copy.join("lib/python3.12/site-packages/pkg/__init__.py")
            .is_file()
    );
    let new = copy.to_str().unwrap();
    let pytest = std::fs::read_to_string(copy.join("bin/pytest")).unwrap();
    assert_eq!(pytest, format!("#!{new}/bin/python\nimport pytest\n"));
    let mode = std::fs::metadata(copy.join("bin/pytest"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "the script stays executable");
    let activate = std::fs::read_to_string(copy.join("bin/activate")).unwrap();
    assert!(
        activate.contains(&format!("VIRTUAL_ENV='{new}'")),
        "{activate}"
    );
    assert!(!activate.contains(&format!("'{old}'")), "{activate}");
}

/// A seeded copy takes the sources and none of the seed's per-node caches: not its CMake
/// `build/` with the seed's `CMakeCache.txt` and `compile_commands.json`, not clangd's index,
/// not SwiftPM's `.build` (#416).
#[test]
fn a_seeded_copy_takes_no_build_directory_holding_the_seeds_paths() {
    let dir = tempfile::tempdir().unwrap();
    let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
    for rel in [
        "CMakeLists.txt",
        "src/main.cpp",
        "build/CMakeCache.txt",
        "build/compile_commands.json",
        ".cache/clangd/index/main.cpp.1A2B.idx",
        "lib/.build/debug.yaml",
        "tests/__pycache__/test_a.cpython-312.pyc",
    ] {
        let path = seed.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, rel).unwrap();
    }
    assert_eq!(copy_tree(&seed, &fresh).unwrap(), 2);
    assert!(fresh.join("CMakeLists.txt").is_file());
    assert!(fresh.join("src/main.cpp").is_file());
    for cache in ["build", ".cache", "lib/.build", "tests/__pycache__"] {
        assert!(!fresh.join(cache).exists(), "{cache} was copied");
    }
}

/// Room for a seed with plenty to spare.
fn roomy() -> Option<DiskSpace> {
    space(u64::MAX / 4, u64::MAX / 2)
}

fn space(free: u64, total: u64) -> Option<DiskSpace> {
    Some(DiskSpace { free, total })
}

/// No build cache, not enough room for two of it, or a copy that would leave less than a
/// fifth of the filesystem free, leaves the new copy without one (#419).
#[test]
fn a_seeded_copy_goes_without_a_build_cache_it_has_no_room_for() {
    let dir = tempfile::tempdir().unwrap();
    let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
    assert_eq!(
        seed_build_cache_within(&seed, &fresh, roomy()).unwrap(),
        None
    );
    let deps = seed.join("target/debug/deps");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("libbig.rlib"), vec![0u8; 1000]).unwrap();
    assert_eq!(
        seed_build_cache_within(&seed, &fresh, space(1999, 5000)).unwrap(),
        None,
        "not twice its size free"
    );
    assert_eq!(seed_build_cache_within(&seed, &fresh, None).unwrap(), None);
    assert_eq!(
        seed_build_cache_within(&seed, &fresh, space(10_000, 46_000)).unwrap(),
        None,
        "9,000 left of 46,000 is under a fifth"
    );
    assert!(!fresh.join("target").exists());
    assert_eq!(
        seed_build_cache_within(&seed, &fresh, space(10_000, 44_000)).unwrap(),
        Some(1000),
        "9,000 left of 44,000 is more than a fifth"
    );
    assert!(
        disk_space(dir.path()).is_some_and(|s| s.free > 0 && s.total >= s.free),
        "{:?}",
        disk_space(dir.path())
    );
    assert!(disk_space(&dir.path().join("not/yet/created")).is_some());
}

/// #1527: seed copies into different worktrees share one ledger. Each copy is checked against
/// the free space less what copies still in flight hold, so three 48 GiB copies on a 980 GiB
/// filesystem with 290 GiB free cannot all pass the 20% floor that each one passes alone
/// (together they would leave 146 GiB, under 196). A finished copy returns its share.
#[test]
fn concurrent_seed_copies_are_checked_against_one_another() {
    use crate::seed_cache::build_cache::reserve_seed;
    let gib = 1u64 << 30;
    let space = || {
        Some(DiskSpace {
            free: 290 * gib,
            total: 980 * gib,
        })
    };
    let first = reserve_seed("target/debug", 48 * gib, space());
    assert!(first.is_some(), "one copy leaves 242 GiB, above the floor");
    assert!(
        reserve_seed("node_modules", 48 * gib, space()).is_none(),
        "a second, while the first still copies, would leave 194 GiB"
    );
    drop(first);
    // Once the first copy is done, its bytes are in what the filesystem reports.
    let after = || {
        Some(DiskSpace {
            free: 242 * gib,
            total: 980 * gib,
        })
    };
    assert!(reserve_seed("node_modules", 20 * gib, after).is_some());
}

/// #1525: a dependency tree is seeded only when the new worktree's package manifest and
/// lockfile beside it are those of the seed copy, and a copy that fails half way leaves
/// nothing behind that would pass for an installed tree.
#[test]
fn a_dependency_tree_is_seeded_only_for_the_same_manifest_and_never_half() {
    use crate::seed_cache::virtualenv::seed_trees;
    use prod_code_protocol::content_hash;
    use std::collections::HashMap;
    let dir = tempfile::tempdir().unwrap();
    let seed = dir.path().join("seed");
    for (rel, text) in [
        ("package.json", "{\"dependencies\":{\"zod\":\"3\"}}"),
        ("package-lock.json", "{\"lockfileVersion\":3}"),
        ("node_modules/zod/index.js", "module.exports = {};"),
    ] {
        std::fs::create_dir_all(seed.join(rel).parent().unwrap()).unwrap();
        std::fs::write(seed.join(rel), text).unwrap();
    }
    let hash = |rel: &str| content_hash(&std::fs::read(seed.join(rel)).unwrap());
    let same: HashMap<&str, u64> = [
        ("package.json", hash("package.json")),
        ("package-lock.json", hash("package-lock.json")),
    ]
    .into();
    let mut other_lock = same.clone();
    other_lock.insert("package-lock.json", hash("package-lock.json") ^ 1);
    let mut no_lock = same.clone();
    no_lock.remove("package-lock.json");
    for (name, incoming, copied) in [
        ("another lockfile", &other_lock, false),
        ("no lockfile", &no_lock, false),
        ("the same inputs", &same, true),
    ] {
        let fresh = dir.path().join(name.replace(' ', "-"));
        let seeded = seed_trees(&seed, &fresh, roomy(), Some(incoming)).unwrap();
        assert_eq!(seeded.is_some(), copied, "{name}");
        assert_eq!(
            fresh.join("node_modules/zod/index.js").is_file(),
            copied,
            "{name}"
        );
    }

    // An unreadable file makes `cp -a` fail after it has written part of the tree.
    let unreadable = seed.join("node_modules/zod/secret.js");
    std::fs::write(&unreadable, "x").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&unreadable).is_ok() {
        eprintln!("skipped the failed-copy half: this user reads a mode-000 file");
        return;
    }
    let broken = dir.path().join("broken");
    assert!(seed_trees(&seed, &broken, roomy(), Some(&same)).is_err());
    assert!(
        !broken.join("node_modules").exists(),
        "the partial tree is removed"
    );
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o644)).unwrap();
}
