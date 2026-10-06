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
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
#[test]
fn prune_rejects_a_symlink_root_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let keep = outside.join("keep.module");
    fs::write(&keep, b"module cache").unwrap();
    let cache_link = temp.path().join("cache-link");
    symlink(&outside, &cache_link).unwrap();

    assert_eq!(
        prune_stale_module_cache_in(&cache_link, Duration::ZERO, 0).unwrap(),
        0
    );
    assert!(keep.exists());
}

#[cfg(unix)]
#[test]
fn cache_directory_rejects_symlinks_and_secures_owned_directories() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempfile::tempdir().unwrap();
    let owned = temp.path().join("owned-cache");
    fs::create_dir(&owned).unwrap();
    fs::set_permissions(&owned, fs::Permissions::from_mode(0o755)).unwrap();
    ensure_cache_dir(&owned).unwrap();
    assert_eq!(
        fs::metadata(&owned).unwrap().permissions().mode() & 0o777,
        0o700
    );

    let target = temp.path().join("external");
    fs::create_dir(&target).unwrap();
    let target_mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
    let link = temp.path().join("cache-link");
    symlink(&target, &link).unwrap();
    assert!(ensure_cache_dir(&link).is_err());
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        target_mode
    );
}

#[cfg(unix)]
#[test]
fn merging_module_cache_skips_symlink_files_and_directories() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    let external = temp.path().join("external");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&external).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(external.join("private.pcm"), b"private").unwrap();
    symlink(&external, src.join("linked-dir")).unwrap();
    symlink(external.join("private.pcm"), src.join("linked-file.pcm")).unwrap();

    merge_cache_files(&src, &dst).unwrap();

    assert!(!dst.join("linked-dir").exists());
    assert!(!dst.join("linked-file.pcm").exists());
    assert!(external.join("private.pcm").exists());
}

#[test]
fn test_swift_module_cache_env_and_path() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("my-custom-swift-cache");

    // Use custom env
    unsafe {
        std::env::set_var(SWIFT_MODULE_CACHE_ENV, &custom_cache);
    }
    let dir = swift_module_cache_dir();
    assert_eq!(dir, custom_cache);
    assert!(dir.is_dir());

    let envs = swift_module_cache_env();
    assert!(
        envs.iter().any(
            |(k, v)| k == "SWIFTPM_MODULECACHE_OVERRIDE" && v == custom_cache.to_str().unwrap()
        )
    );
    assert!(
        envs.iter()
            .any(|(k, v)| k == "SWIFT_MODULE_CACHE_PATH" && v == custom_cache.to_str().unwrap())
    );
    assert!(
        envs.iter()
            .any(|(k, v)| k == "CLANG_MODULE_CACHE_PATH" && v == custom_cache.to_str().unwrap())
    );

    unsafe {
        std::env::remove_var(SWIFT_MODULE_CACHE_ENV);
    }
}

#[test]
fn test_relocate_swiftpm_workspace_state_component_boundaries() {
    let from_root = "/Users/developer/prod-code";
    let to_root = "/tmp/ram-disk/prod-code-worktree-1";

    let sample_json = r#"{
  "object": {
    "artifacts": [],
    "dependencies": [
      {
        "packageRef": {
          "identity": "myswiftpkg",
          "kind": "localSourceControl",
          "location": "/Users/developer/prod-code/packages/myswiftpkg",
          "name": "MySwiftPkg"
        },
        "state": {
          "name": "localSourceControl",
          "path": "/Users/developer/prod-code/.build/checkouts/myswiftpkg"
        },
        "subpath": "myswiftpkg"
      },
      {
        "packageRef": {
          "identity": "external-dep",
          "kind": "localSourceControl",
          "location": "/Users/developer/prod-code-external-deps/external-dep",
          "name": "ExternalDep"
        },
        "state": {
          "name": "localSourceControl",
          "path": "/Users/developer/prod-code-sibling/external-dep"
        },
        "subpath": "external-dep"
      }
    ]
  },
  "version": 1
}"#;

    let relocated = relocate_swiftpm_workspace_state(sample_json, from_root, to_root).unwrap();

    // Target path under from_root should be relocated
    assert!(relocated.contains("/tmp/ram-disk/prod-code-worktree-1/packages/myswiftpkg"));
    assert!(relocated.contains("/tmp/ram-disk/prod-code-worktree-1/.build/checkouts/myswiftpkg"));

    // Sibling paths starting with prefix substrings MUST NOT be changed
    assert!(relocated.contains("/Users/developer/prod-code-external-deps/external-dep"));
    assert!(relocated.contains("/Users/developer/prod-code-sibling/external-dep"));
    assert!(!relocated.contains("/tmp/ram-disk/prod-code-worktree-1-external-deps"));
    assert!(!relocated.contains("/tmp/ram-disk/prod-code-worktree-1-sibling"));
}

#[test]
fn test_find_swift_packages_root_and_nested() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // Root package
    fs::write(root.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();

    // Nested package in clients/macos/ProdUI
    let macos_pkg = root.join("clients").join("macos").join("ProdUI");
    fs::create_dir_all(&macos_pkg).unwrap();
    fs::write(
        macos_pkg.join("Package.swift"),
        "// swift-tools-version:5.9\n",
    )
    .unwrap();

    // Ignored package inside .build or target
    let ignored_pkg = root.join(".build").join("checkouts").join("ignored");
    fs::create_dir_all(&ignored_pkg).unwrap();
    fs::write(
        ignored_pkg.join("Package.swift"),
        "// swift-tools-version:5.9\n",
    )
    .unwrap();

    let found = find_swift_packages(root);
    assert_eq!(found.len(), 2);
    assert_eq!(found[0], PathBuf::from(""));
    assert_eq!(found[1], PathBuf::from("clients/macos/ProdUI"));
}

#[test]
fn test_seed_swift_worktree_basic() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let from = temp.path().join("seed-workspace");
    let to = temp.path().join("new-worktree");

    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();

    let from_build = from.join(".build");
    fs::create_dir_all(&from_build).unwrap();

    // Mock checkouts
    let checkout_dir = from_build.join("checkouts").join("MyLib");
    fs::create_dir_all(&checkout_dir).unwrap();
    fs::write(checkout_dir.join("MyLib.swift"), "public let x = 42;\n").unwrap();

    // Mock repositories
    let repo_dir = from_build.join("repositories").join("MyLib-hash");
    fs::create_dir_all(&repo_dir).unwrap();
    fs::write(repo_dir.join("config"), "bare git repo mock\n").unwrap();

    // Mock workspace-state.json
    let from_str = from.to_str().unwrap();
    let state_content = format!(
        r#"{{"object":{{"artifacts":[],"dependencies":[{{"state":{{"path":"{from_str}/.build/checkouts/MyLib"}}}}]}}}}"#
    );
    fs::write(from_build.join("workspace-state.json"), state_content).unwrap();

    // Run seed
    let result = seed_swift_worktree(&from, &to).unwrap();
    assert!(result.is_some());

    let to_build = to.join(".build");
    assert!(to_build.is_dir());

    // Verify checkouts copied
    assert_eq!(
        fs::read_to_string(to_build.join("checkouts").join("MyLib").join("MyLib.swift")).unwrap(),
        "public let x = 42;\n"
    );

    // Verify repositories copied
    assert!(
        to_build
            .join("repositories")
            .join("MyLib-hash")
            .join("config")
            .is_file()
    );

    // Verify workspace-state.json relocated
    let to_state = fs::read_to_string(to_build.join("workspace-state.json")).unwrap();
    let to_str = to.to_str().unwrap();
    assert!(to_state.contains(&format!("{to_str}/.build/checkouts/MyLib")));
    assert!(!to_state.contains(&format!("{from_str}/.build/checkouts/MyLib")));

    // Verify ModuleCache symlink
    let to_module_cache = to_build.join("ModuleCache");
    assert!(
        fs::symlink_metadata(&to_module_cache)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let link_target = fs::read_link(&to_module_cache).unwrap();
    assert_eq!(link_target, swift_module_cache_dir());
}

#[test]
fn test_seed_swift_worktree_skips_non_swift_projects() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let from = temp.path().join("rust-workspace");
    let to = temp.path().join("new-worktree");

    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("Cargo.toml"), "[package]\nname=\"foo\"\n").unwrap();

    let result = seed_swift_worktree(&from, &to).unwrap();
    assert!(result.is_none());
    assert!(!to.exists());
}

#[test]
fn test_prune_stale_module_cache() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("prune-cache");
    unsafe {
        std::env::set_var(SWIFT_MODULE_CACHE_ENV, &custom_cache);
    }

    ensure_cache_dir(&custom_cache).unwrap();

    let old_file = custom_cache.join("old-module.pcm");
    fs::write(&old_file, vec![0u8; 1024]).unwrap();

    let new_file = custom_cache.join("new-module.pcm");
    fs::write(&new_file, vec![0u8; 1024]).unwrap();

    // Pruning with max_size_bytes=1500 will evict one file
    let pruned = prune_stale_module_cache(Duration::from_secs(3600), 1500).unwrap();
    assert_eq!(pruned, 1);

    // Direct directory pruning
    fs::write(&old_file, vec![0u8; 1024]).unwrap();
    let pruned_in =
        prune_stale_module_cache_in(&custom_cache, Duration::from_secs(3600), 1500).unwrap();
    assert_eq!(pruned_in, 1);

    unsafe {
        std::env::remove_var(SWIFT_MODULE_CACHE_ENV);
    }
}
