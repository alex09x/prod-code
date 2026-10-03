#![cfg(unix)]

use prod_code_gateway::{polyglot_compiler_cache_env, DiskSpace};
use prod_code_gateway::swift_cache::{
    find_swift_packages, prune_stale_module_cache_in, relocate_swiftpm_workspace_state,
    seed_swift_worktree, seed_swift_worktree_within, swift_module_cache_dir, swift_module_cache_env,
    SWIFT_MODULE_CACHE_ENV,
};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_swift_module_cache_dir_and_env_resolution() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("isolated-swift-module-cache");

    unsafe {
        std::env::set_var(SWIFT_MODULE_CACHE_ENV, &custom_cache);
    }

    let cache_dir = swift_module_cache_dir();
    assert_eq!(cache_dir, custom_cache);
    assert!(cache_dir.is_dir());

    let envs = swift_module_cache_env();
    let cache_str = custom_cache.to_str().unwrap();

    assert!(envs.iter().any(|(k, v)| k == "SWIFTPM_MODULECACHE_OVERRIDE" && v == cache_str));
    assert!(envs.iter().any(|(k, v)| k == "SWIFT_MODULE_CACHE_PATH" && v == cache_str));
    assert!(envs.iter().any(|(k, v)| k == "CLANG_MODULE_CACHE_PATH" && v == cache_str));

    unsafe {
        std::env::remove_var(SWIFT_MODULE_CACHE_ENV);
    }
}

#[test]
fn test_relocate_swiftpm_workspace_state_preserves_sibling_paths() {
    let from_root = "/srv/workspaces/tako-app";
    let to_root = "/srv/workspaces/tako-app--wt-42";

    let state_json = r#"{
  "object": {
    "artifacts": [
      {
        "packageRef": {
          "identity": "binary-kit",
          "name": "BinaryKit"
        },
        "path": "/srv/workspaces/tako-app/.build/artifacts/binary-kit"
      }
    ],
    "dependencies": [
      {
        "packageRef": {
          "identity": "tako-core",
          "kind": "localSourceControl",
          "location": "/srv/workspaces/tako-app/packages/tako-core",
          "name": "TakoCore"
        },
        "state": {
          "name": "localSourceControl",
          "path": "/srv/workspaces/tako-app/.build/checkouts/tako-core"
        },
        "subpath": "tako-core"
      },
      {
        "packageRef": {
          "identity": "sibling-dep",
          "kind": "localSourceControl",
          "location": "/srv/workspaces/tako-app-sibling/dep",
          "name": "SiblingDep"
        },
        "state": {
          "name": "localSourceControl",
          "path": "/srv/workspaces/tako-app-external/dep"
        },
        "subpath": "sibling-dep"
      }
    ]
  },
  "version": 1
}"#;

    let relocated = relocate_swiftpm_workspace_state(state_json, from_root, to_root).unwrap();

    // Check workspace-owned paths relocated
    assert!(
        relocated.contains("/srv/workspaces/tako-app--wt-42/.build/artifacts/binary-kit"),
        "artifacts path must be relocated"
    );
    assert!(
        relocated.contains("/srv/workspaces/tako-app--wt-42/packages/tako-core"),
        "package location must be relocated"
    );
    assert!(
        relocated.contains("/srv/workspaces/tako-app--wt-42/.build/checkouts/tako-core"),
        "checkout state path must be relocated"
    );

    // Check sibling paths with prefix substrings are preserved untouched
    assert!(
        relocated.contains("/srv/workspaces/tako-app-sibling/dep"),
        "sibling repository location must remain untouched"
    );
    assert!(
        relocated.contains("/srv/workspaces/tako-app-external/dep"),
        "external checkout path must remain untouched"
    );
    assert!(
        !relocated.contains("/srv/workspaces/tako-app--wt-42-sibling"),
        "sibling directory prefix must never be rewritten"
    );
    assert!(
        !relocated.contains("/srv/workspaces/tako-app--wt-42-external"),
        "external directory prefix must never be rewritten"
    );
}

#[test]
fn test_seed_swift_worktree_checkouts_repositories_artifacts_and_module_cache() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("shared-module-cache");

    unsafe {
        std::env::set_var(SWIFT_MODULE_CACHE_ENV, &custom_cache);
    }

    let origin = temp.path().join("origin-workspace");
    let worktree = temp.path().join("worktree-workspace");

    fs::create_dir_all(&origin).unwrap();
    fs::write(
        origin.join("Package.swift"),
        "// swift-tools-version: 5.10\nimport PackageDescription\nlet package = Package(name: \"App\");\n",
    )
    .unwrap();

    // Create .build layout in origin
    let origin_build = origin.join(".build");
    fs::create_dir_all(&origin_build).unwrap();

    // 1. Checkouts
    let checkout_dir = origin_build.join("checkouts").join("swift-algorithms");
    fs::create_dir_all(&checkout_dir).unwrap();
    fs::write(
        checkout_dir.join("Algorithms.swift"),
        "public struct Chunked {}\n",
    )
    .unwrap();

    // 2. Bare repositories
    let repo_dir = origin_build.join("repositories").join("swift-algorithms-hash");
    fs::create_dir_all(&repo_dir).unwrap();
    fs::write(repo_dir.join("config"), "bare git repo\n").unwrap();

    // 3. Artifacts
    let artifact_dir = origin_build.join("artifacts").join("MyBinaryTarget.xcframework");
    fs::create_dir_all(&artifact_dir).unwrap();
    fs::write(artifact_dir.join("Info.plist"), "<plist></plist>\n").unwrap();

    // 4. Existing local ModuleCache files
    let local_cache = origin_build.join("ModuleCache");
    fs::create_dir_all(&local_cache).unwrap();
    fs::write(local_cache.join("Foundation-1234.pcm"), b"precompiled foundation pcm").unwrap();

    // 5. workspace-state.json
    let origin_str = origin.to_str().unwrap();
    let state_content = format!(
        r#"{{"object":{{"artifacts":[],"dependencies":[{{"state":{{"path":"{origin_str}/.build/checkouts/swift-algorithms"}}}}]}}}}"#
    );
    fs::write(origin_build.join("workspace-state.json"), state_content).unwrap();

    // 6. Nested package in clients/macos/ProdUI
    let nested_pkg = origin.join("clients").join("macos").join("ProdUI");
    fs::create_dir_all(&nested_pkg).unwrap();
    fs::write(
        nested_pkg.join("Package.swift"),
        "// swift-tools-version: 5.10\nlet package = Package(name: \"ProdUI\");\n",
    )
    .unwrap();
    let nested_build = nested_pkg.join(".build");
    fs::create_dir_all(&nested_build).unwrap();
    fs::write(nested_build.join("workspace-state.json"), "{}").unwrap();

    // Verify package discovery
    let packages = find_swift_packages(&origin);
    assert_eq!(packages.len(), 2);
    assert_eq!(packages[0], PathBuf::from(""));
    assert_eq!(packages[1], PathBuf::from("clients/macos/ProdUI"));

    // Run seed
    let result = seed_swift_worktree(&origin, &worktree).unwrap();
    assert!(result.is_some(), "seed_swift_worktree must seed files");

    let wt_build = worktree.join(".build");
    assert!(wt_build.is_dir());

    // 1. Assert checkouts preserved
    assert_eq!(
        fs::read_to_string(wt_build.join("checkouts").join("swift-algorithms").join("Algorithms.swift")).unwrap(),
        "public struct Chunked {}\n"
    );

    // 2. Assert repositories preserved
    assert_eq!(
        fs::read_to_string(wt_build.join("repositories").join("swift-algorithms-hash").join("config")).unwrap(),
        "bare git repo\n"
    );

    // 3. Assert artifacts preserved
    assert!(wt_build.join("artifacts").join("MyBinaryTarget.xcframework").join("Info.plist").is_file());

    // 4. Assert workspace-state.json relocated
    let wt_state = fs::read_to_string(wt_build.join("workspace-state.json")).unwrap();
    let wt_str = worktree.to_str().unwrap();
    assert!(wt_state.contains(&format!("{wt_str}/.build/checkouts/swift-algorithms")));
    assert!(!wt_state.contains(&format!("{origin_str}/.build/checkouts/swift-algorithms")));

    // 5. Assert ModuleCache symlink points to shared module cache
    let wt_module_cache = wt_build.join("ModuleCache");
    assert!(fs::symlink_metadata(&wt_module_cache).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_link(&wt_module_cache).unwrap(), custom_cache);

    // 6. Assert existing local .pcm cache was merged into shared cache
    assert!(custom_cache.join("Foundation-1234.pcm").is_file());
    assert_eq!(
        fs::read(custom_cache.join("Foundation-1234.pcm")).unwrap(),
        b"precompiled foundation pcm"
    );

    // 7. Assert nested package has its ModuleCache symlink established
    let nested_wt_cache = worktree.join("clients").join("macos").join("ProdUI").join(".build").join("ModuleCache");
    assert!(fs::symlink_metadata(&nested_wt_cache).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_link(&nested_wt_cache).unwrap(), custom_cache);

    unsafe {
        std::env::remove_var(SWIFT_MODULE_CACHE_ENV);
    }
}

#[test]
fn test_polyglot_compiler_cache_env_contains_swift_module_cache() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("polyglot-swift-cache");

    unsafe {
        std::env::set_var(SWIFT_MODULE_CACHE_ENV, &custom_cache);
    }

    let ws = temp.path().join("repo");
    fs::create_dir_all(&ws).unwrap();

    let envs = polyglot_compiler_cache_env(&ws, false, None);
    let cache_str = custom_cache.to_str().unwrap();

    assert!(envs.iter().any(|(k, v)| k == "SWIFTPM_MODULECACHE_OVERRIDE" && v == cache_str));
    assert!(envs.iter().any(|(k, v)| k == "SWIFT_MODULE_CACHE_PATH" && v == cache_str));
    assert!(envs.iter().any(|(k, v)| k == "CLANG_MODULE_CACHE_PATH" && v == cache_str));

    unsafe {
        std::env::remove_var(SWIFT_MODULE_CACHE_ENV);
    }
}

#[test]
fn test_prune_stale_module_cache_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    fs::create_dir_all(&cache_dir).unwrap();

    // Create mock modules
    let mod1 = cache_dir.join("ModA.pcm");
    let mod2 = cache_dir.join("ModB.pcm");
    let mod3 = cache_dir.join("ModC.pcm");
    let lock_file = cache_dir.join("ModA.pcm.lock");

    fs::write(&mod1, vec![1u8; 1000]).unwrap();
    fs::write(&mod2, vec![2u8; 1000]).unwrap();
    fs::write(&mod3, vec![3u8; 1000]).unwrap();
    fs::write(&lock_file, b"lock content").unwrap();

    // Prune with max_size_bytes 2500 should evict 1 module and preserve lock file
    let evicted = prune_stale_module_cache_in(&cache_dir, Duration::from_secs(3600), 2500).unwrap();
    assert_eq!(evicted, 1);
    assert!(lock_file.is_file(), "lock files must never be pruned");
}

#[test]
fn test_seed_swift_worktree_skips_heavy_dirs_when_disk_space_insufficient() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("shared-module-cache-budget");

    unsafe {
        std::env::set_var(SWIFT_MODULE_CACHE_ENV, &custom_cache);
    }

    let origin = temp.path().join("origin-workspace");
    let worktree = temp.path().join("worktree-workspace");

    fs::create_dir_all(&origin).unwrap();
    fs::write(
        origin.join("Package.swift"),
        "// swift-tools-version: 5.10\nimport PackageDescription\nlet package = Package(name: \"App\");\n",
    )
    .unwrap();

    let origin_build = origin.join(".build");
    fs::create_dir_all(&origin_build).unwrap();

    let checkout_dir = origin_build.join("checkouts").join("heavy-lib");
    fs::create_dir_all(&checkout_dir).unwrap();
    fs::write(checkout_dir.join("Heavy.swift"), vec![b'A'; 100_000]).unwrap();

    let origin_str = origin.to_str().unwrap();
    let state_content = format!(
        r#"{{"object":{{"artifacts":[],"dependencies":[{{"state":{{"path":"{origin_str}/.build/checkouts/heavy-lib"}}}}]}}}}"#
    );
    fs::write(origin_build.join("workspace-state.json"), state_content).unwrap();

    // Pass a DiskSpace budget where free space is too small to fit the heavy copy (100 bytes free, total 1_000_000)
    let restricted_space = Some(DiskSpace {
        free: 100,
        total: 1_000_000,
    });

    let result = seed_swift_worktree_within(&origin, &worktree, restricted_space).unwrap();
    assert!(result.is_some());

    let wt_build = worktree.join(".build");
    assert!(wt_build.is_dir());

    // Heavy checkouts must NOT be copied because of disk space budget
    assert!(!wt_build.join("checkouts").exists(), "heavy checkouts must be skipped when disk space is tight");

    // But ModuleCache symlink MUST still be established
    let wt_module_cache = wt_build.join("ModuleCache");
    assert!(fs::symlink_metadata(&wt_module_cache).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_link(&wt_module_cache).unwrap(), custom_cache);

    // And workspace-state.json must still be relocated
    let wt_state = fs::read_to_string(wt_build.join("workspace-state.json")).unwrap();
    let wt_str = worktree.to_str().unwrap();
    assert!(wt_state.contains(&format!("{wt_str}/.build/checkouts/heavy-lib")));

    unsafe {
        std::env::remove_var(SWIFT_MODULE_CACHE_ENV);
    }
}

#[test]
fn test_prune_stale_module_cache_ignores_symlink_directories_and_external_targets() {
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    let external_dir = temp.path().join("external-system-dir");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::create_dir_all(&external_dir).unwrap();

    // Create an important external file
    let external_file = external_dir.join("critical_module.pcm");
    fs::write(&external_file, b"do not delete me!").unwrap();

    // Symlink inside cache pointing to external directory
    let symlinked_dir = cache_dir.join("symlink_to_external");
    std::os::unix::fs::symlink(&external_dir, &symlinked_dir).unwrap();

    // Create a regular cache file inside cache_dir that is over budget
    let regular_file = cache_dir.join("regular.pcm");
    fs::write(&regular_file, vec![1u8; 5000]).unwrap();

    // Prune cache with max_size_bytes 100
    let evicted = prune_stale_module_cache_in(&cache_dir, Duration::from_secs(3600), 100).unwrap();
    assert_eq!(evicted, 1);
    assert!(!regular_file.exists(), "regular cache file should be pruned");

    // Critical assertion: external file MUST NOT be deleted, and directory symlink must not be traversed or removed
    assert!(external_file.is_file(), "external file outside cache root must never be deleted by pruner");
    assert!(symlinked_dir.exists(), "directory symlink itself must not be deleted");
}
