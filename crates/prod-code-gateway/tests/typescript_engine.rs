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

use prod_code_gateway::{polyglot_compiler_cache_env, DiskSpace};
use prod_code_gateway::ts_cache::{
    approved_target, build_approved_roots, find_project_types, has_declaration_files,
    is_typescript_project, merge_types, prune_stale_types_cache_in,
    prune_stale_types_cache_with_grace, ts_types_cache_dir, ts_types_cache_env,
    seed_typescript_worktree, seed_typescript_worktree_within, tree_size,
    TS_TYPES_CACHE_ENV,
};
use std::fs;
use std::time::Duration;

static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_ts_types_cache_dir_and_env_resolution() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("isolated-ts-types");

    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &custom_cache);
    }

    let cache_dir = ts_types_cache_dir();
    assert_eq!(cache_dir, custom_cache);
    assert!(cache_dir.is_dir());

    let envs = ts_types_cache_env();
    let cache_str = custom_cache.to_str().unwrap();

    assert!(envs.iter().any(|(k, v)| k == TS_TYPES_CACHE_ENV && v == cache_str));
    assert!(envs.iter().any(|(k, v)| k == "TS_TYPES_CACHE" && v == cache_str));

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_polyglot_compiler_cache_env_contains_ts_types_cache() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("polyglot-ts-cache");

    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &custom_cache);
    }

    let ws = temp.path().join("repo");
    fs::create_dir_all(&ws).unwrap();

    let envs = polyglot_compiler_cache_env(&ws, false, None);
    let cache_str = custom_cache.to_str().unwrap();

    assert!(envs.iter().any(|(k, v)| k == TS_TYPES_CACHE_ENV && v == cache_str));
    assert!(envs.iter().any(|(k, v)| k == "TS_TYPES_CACHE" && v == cache_str));

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_is_typescript_project_detection() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    assert!(!is_typescript_project(root));

    // package.json detection
    fs::write(root.join("package.json"), "{\"name\":\"foo\"}\n").unwrap();
    assert!(is_typescript_project(root));
    fs::remove_file(root.join("package.json")).unwrap();

    // tsconfig.json detection
    fs::write(root.join("tsconfig.json"), "{}\n").unwrap();
    assert!(is_typescript_project(root));
    fs::remove_file(root.join("tsconfig.json")).unwrap();

    // jsconfig.json detection
    fs::write(root.join("jsconfig.json"), "{}\n").unwrap();
    assert!(is_typescript_project(root));
    fs::remove_file(root.join("jsconfig.json")).unwrap();

    // Nested source file detection
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("main.ts"), "export const a = 1;\n").unwrap();
    assert!(is_typescript_project(root));
}

#[test]
fn test_seed_typescript_worktree_at_types_and_symlink() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("shared-ts-types");
    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &custom_cache);
    }

    let from = temp.path().join("origin-project");
    let to = temp.path().join("worktree-project");
    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("package.json"), "{\"name\": \"demo\"}\n").unwrap();

    // node_modules/@types/react with index.d.ts
    let react_types = from.join("node_modules").join("@types").join("react");
    fs::create_dir_all(&react_types).unwrap();
    fs::write(react_types.join("index.d.ts"), "export declare function useState<T>(init: T): [T, (v: T) => void];\n").unwrap();
    fs::write(react_types.join("package.json"), "{\"name\": \"@types/react\", \"types\": \"index.d.ts\"}\n").unwrap();

    let result = seed_typescript_worktree(&from, &to).unwrap();
    assert!(result.is_some());

    // Verify each worktree gets a version-isolated view under the configured shared-cache root.
    let to_at_types = to.join("node_modules").join("@types");
    let meta = fs::symlink_metadata(&to_at_types).unwrap();
    assert!(meta.file_type().is_symlink());
    let target = fs::read_link(&to_at_types).unwrap();
    assert!(target.starts_with(&custom_cache));

    let cached_react = target.join("react").join("index.d.ts");
    assert!(cached_react.is_file());
    assert!(fs::read_to_string(&cached_react).unwrap().contains("useState"));

    // Verify to sees the cached declaration
    let to_react_dts = to_at_types.join("react").join("index.d.ts");
    assert!(to_react_dts.is_file());
    assert!(fs::read_to_string(&to_react_dts).unwrap().contains("useState"));

    // A changed resolver lock must select a new cache namespace and retarget this worktree.
    fs::write(from.join("package-lock.json"), "{\"lockfileVersion\": 3, \"version\": 1}\n")
        .unwrap();
    fs::write(
        react_types.join("index.d.ts"),
        "export declare function useStateV2<T>(init: T): [T, (v: T) => void];\n",
    )
    .unwrap();
    assert!(seed_typescript_worktree(&from, &to).unwrap().is_some());
    let next_target = fs::read_link(&to_at_types).unwrap();
    assert_ne!(next_target, target, "the new lock must not reuse the old cache view");
    let next_content =
        fs::read_to_string(next_target.join("react").join("index.d.ts")).unwrap();
    assert!(next_content.contains("useStateV2"), "{next_content}");

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_seed_typescript_worktree_monorepo_discovery() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("monorepo-cache");
    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &custom_cache);
    }

    let from = temp.path().join("monorepo");
    let to = temp.path().join("worktree-monorepo");
    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("package.json"), "{\"workspaces\": [\"packages/*\"]}\n").unwrap();

    // Nested package: packages/web/node_modules/@types/node
    let web_node_types = from.join("packages").join("web").join("node_modules").join("@types").join("node");
    fs::create_dir_all(&web_node_types).unwrap();
    fs::write(web_node_types.join("index.d.ts"), "declare module 'fs' { export function readFileSync(): string; }\n").unwrap();

    // Project-level custom types: types/global.d.ts
    let custom_types = from.join("types");
    fs::create_dir_all(&custom_types).unwrap();
    fs::write(custom_types.join("global.d.ts"), "declare const __VERSION__: string;\n").unwrap();

    let discovered = find_project_types(&from);
    assert!(discovered.iter().any(|d| d.ends_with("@types")));
    assert!(discovered.iter().any(|d| d.ends_with("types")));

    let result = seed_typescript_worktree(&from, &to).unwrap();
    assert!(result.is_some());

    assert!(custom_cache.join("node").join("index.d.ts").is_file());
    assert!(custom_cache.join("types").join("global.d.ts").is_file() || custom_cache.join("global.d.ts").is_file());

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_seed_typescript_worktree_deduplicates_existing_dir() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("dedup-cache");
    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &custom_cache);
    }

    let from = temp.path().join("from");
    let to = temp.path().join("to");
    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("package.json"), "{}\n").unwrap();

    // Destination already has a concrete directory node_modules/@types (e.g. from seed_dependency_trees)
    let to_at_types = to.join("node_modules").join("@types");
    let to_express = to_at_types.join("express");
    fs::create_dir_all(&to_express).unwrap();
    fs::write(to_express.join("index.d.ts"), "export declare function express(): any;\n").unwrap();

    let result = seed_typescript_worktree(&from, &to).unwrap();
    assert!(result.is_some());

    // Verify it was merged into a version-isolated view and replaced with a symlink.
    let meta = fs::symlink_metadata(&to_at_types).unwrap();
    assert!(meta.file_type().is_symlink());
    let cache_view = fs::read_link(&to_at_types).unwrap();
    assert!(cache_view.starts_with(&custom_cache));
    assert!(cache_view.join("express").join("index.d.ts").is_file());

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_seed_typescript_worktree_respects_disk_budget() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("budget-cache");
    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &custom_cache);
    }

    let from = temp.path().join("from");
    let to = temp.path().join("to");
    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("package.json"), "{}\n").unwrap();

    let react_types = from.join("node_modules").join("@types").join("large");
    fs::create_dir_all(&react_types).unwrap();
    fs::write(react_types.join("index.d.ts"), "a".repeat(100_000)).unwrap();

    // Zero disk headroom
    let zero_space = Some(DiskSpace {
        free: 0,
        total: 1_000_000,
    });

    let _ = seed_typescript_worktree_within(&from, &to, zero_space).unwrap();
    // Cache directory should not contain the large stub since disk headroom was 0
    assert!(!custom_cache.join("large").join("index.d.ts").exists());

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_seed_typescript_worktree_preserves_client_type_roots() {
    let temp = tempfile::tempdir().unwrap();
    let tsconfig = temp.path().join("tsconfig.json");

    let initial = serde_json::json!({
        "compilerOptions": {
            "target": "ESNext",
            "typeRoots": ["./custom_types"]
        }
    });
    fs::write(&tsconfig, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

    let from = temp.path().join("dummy");
    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("package.json"), "{}").unwrap();

    let _ = seed_typescript_worktree(&from, temp.path()).unwrap();

    let content = fs::read_to_string(&tsconfig).unwrap();
    let val: serde_json::Value = serde_json::from_str(&content).unwrap();
    let type_roots = val["compilerOptions"]["typeRoots"].as_array().unwrap();
    assert_eq!(type_roots.len(), 1);
    assert!(type_roots.iter().any(|v| v.as_str() == Some("./custom_types")));
}

#[test]
fn test_symlink_cycle_protection_in_tree_size_and_merge() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("cycle_dir");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("test.d.ts"), "declare const x: number;\n").unwrap();

    // Create cyclic symlink: dir/loop -> dir
    let loop_link = dir.join("loop");
    let _ = std::os::unix::fs::symlink(&dir, &loop_link);

    // tree_size must terminate and ignore symlink
    let size = tree_size(&dir);
    assert!(size > 0);

    // merge_types must terminate and ignore symlink
    let dst = temp.path().join("dst");
    let merged = merge_types(&dir, &dst).unwrap();
    assert!(merged > 0);
    assert!(!dst.join("loop").exists());
}

#[test]
fn test_prune_stale_types_cache_evicts_old_and_respects_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("prune-cache");
    fs::create_dir_all(&cache_dir).unwrap();

    let file1 = cache_dir.join("file1.d.ts");
    let file2 = cache_dir.join("file2.d.ts");

    fs::write(&file1, "declare const a: string;").unwrap();
    fs::write(&file2, "declare const b: string;").unwrap();

    // 1. Prune with 0 age limit: should evict everything
    let evicted = prune_stale_types_cache_in(&cache_dir, Duration::ZERO, 100_000).unwrap();
    assert_eq!(evicted, 2);
    assert!(!file1.exists());
    assert!(!file2.exists());

    // 2. Capacity-based eviction
    fs::write(&file1, "a".repeat(1000)).unwrap();
    fs::write(&file2, "b".repeat(1000)).unwrap();

    // Capacity limit 1500 bytes: one 1000-byte file must be evicted
    let evicted = prune_stale_types_cache_in(&cache_dir, Duration::from_secs(3600), 1500).unwrap();
    assert_eq!(evicted, 1);
    let remaining = [file1.exists(), file2.exists()].iter().filter(|&&e| e).count();
    assert_eq!(remaining, 1);
}

#[test]
fn test_prune_stale_types_cache_never_unlinks_locked_temp_file_even_if_old() {
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("locked-prune-cache");
    fs::create_dir_all(&cache_dir).unwrap();

    let old_ts = 1_550_000_000_000_000_000u128; // Epoch year ~2019/2020 nanos
    let tmp_locked = cache_dir.join(format!(".tmp-ts-12345-42-{old_ts:x}"));

    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&tmp_locked)
        .unwrap();

    // Acquire exclusive OS lock
    use std::os::unix::io::AsRawFd;
    let ret = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(ret, 0);

    // Prune with 0-sec grace period
    let removed = prune_stale_types_cache_with_grace(&cache_dir, Duration::ZERO, 1_000_000, Duration::ZERO).unwrap();
    assert_eq!(removed, 0);
    assert!(tmp_locked.exists(), "Locked temporary file must never be unlinked!");

    // Release lock
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN); }
    drop(f);

    // Now unlocked: prune should unlink the abandoned temp file
    let removed_after = prune_stale_types_cache_with_grace(&cache_dir, Duration::ZERO, 1_000_000, Duration::ZERO).unwrap();
    assert_eq!(removed_after, 1);
    assert!(!tmp_locked.exists());
}

#[test]
fn test_seed_typescript_worktree_pnpm_symlink_package() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("ts-cache-pnpm");
    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &cache_dir);
    }

    let from = temp.path().join("from");
    let to = temp.path().join("to");
    fs::create_dir_all(&from).unwrap();
    fs::create_dir_all(&to).unwrap();
    fs::write(from.join("package.json"), "{}").unwrap();

    // Create pnpm virtual store structure:
    // from/node_modules/.pnpm/@types+node@20.11.0/node_modules/@types/node
    let pnpm_pkg = from
        .join("node_modules")
        .join(".pnpm")
        .join("@types+node@20.11.0")
        .join("node_modules")
        .join("@types")
        .join("node");
    fs::create_dir_all(&pnpm_pkg).unwrap();
    fs::write(pnpm_pkg.join("index.d.ts"), "declare const pnpmProcess: any;\n").unwrap();
    fs::write(pnpm_pkg.join("package.json"), "{\"name\":\"@types/node\"}\n").unwrap();

    // Create from/node_modules/@types/node as symlink into pnpm store
    let at_types = from.join("node_modules").join("@types");
    fs::create_dir_all(&at_types).unwrap();
    let link_target = at_types.join("node");
    let _ = std::os::unix::fs::symlink(&pnpm_pkg, &link_target);

    // Verify tree_size measures the dereferenced types (> 0)
    let size = tree_size(&at_types);
    assert!(size > 0, "tree_size must dereference valid package symlinks");

    // Seed worktree
    let result = seed_typescript_worktree(&from, &to).unwrap();
    assert!(result.is_some());

    // to/node_modules/@types must be symlink to shared cache
    let to_at_types = to.join("node_modules").join("@types");
    assert!(to_at_types.exists());
    let meta = fs::symlink_metadata(&to_at_types).unwrap();
    assert!(meta.file_type().is_symlink());

    // to/node_modules/@types/node/index.d.ts must resolve and have content
    let to_index = to_at_types.join("node").join("index.d.ts");
    assert!(to_index.exists());
    let content = fs::read_to_string(&to_index).unwrap();
    assert!(content.contains("pnpmProcess"));

    // Cache must have concrete files beneath this worktree's isolated view.
    let cache_view = fs::read_link(&to_at_types).unwrap();
    assert!(cache_view.starts_with(&cache_dir));
    let cache_index = cache_view.join("node").join("index.d.ts");
    assert!(cache_index.is_file());

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_seed_typescript_worktree_deduplicates_pnpm_concrete_at_types_dir() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("ts-cache-pnpm-dedup");
    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &cache_dir);
    }

    let from = temp.path().join("from");
    let to = temp.path().join("to");
    fs::create_dir_all(&from).unwrap();
    fs::create_dir_all(&to).unwrap();
    fs::write(from.join("package.json"), "{}").unwrap();

    // Create destination worktree with a concrete node_modules/@types containing a pnpm symlink
    let pnpm_react = to
        .join("node_modules")
        .join(".pnpm")
        .join("@types+react@18.2.0")
        .join("node_modules")
        .join("@types")
        .join("react");
    fs::create_dir_all(&pnpm_react).unwrap();
    fs::write(pnpm_react.join("index.d.ts"), "export declare function useState(): void;\n").unwrap();

    let to_at_types = to.join("node_modules").join("@types");
    fs::create_dir_all(&to_at_types).unwrap();
    let react_link = to_at_types.join("react");
    let _ = std::os::unix::fs::symlink(&pnpm_react, &react_link);

    // Run seeding: to_at_types should be merged into cache and deduplicated to a symlink
    let _ = seed_typescript_worktree(&from, &to).unwrap();

    // to_at_types must now be a symlink to shared cache
    let meta = fs::symlink_metadata(&to_at_types).unwrap();
    assert!(meta.file_type().is_symlink());

    // Declarations must NOT be dropped!
    let to_react_index = to_at_types.join("react").join("index.d.ts");
    assert!(to_react_index.exists(), "Type declarations must not be dropped after deduplication");
    let content = fs::read_to_string(&to_react_index).unwrap();
    assert!(content.contains("useState"));

    // Cache view must contain react/index.d.ts as a concrete file.
    let cache_view = fs::read_link(&to_at_types).unwrap();
    assert!(cache_view.starts_with(&cache_dir));
    let cache_react_index = cache_view.join("react").join("index.d.ts");
    assert!(cache_react_index.is_file());

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_seed_typescript_worktree_rejects_out_of_root_symlinks() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("ts-cache-security");
    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &cache_dir);
    }

    // External directory simulating sensitive host files
    let outside_dir = temp.path().join("outside_host_data");
    fs::create_dir_all(&outside_dir).unwrap();
    fs::write(outside_dir.join("secret_credentials.json"), "{\"token\": \"SECRET_HOST_TOKEN\"}\n").unwrap();
    fs::write(outside_dir.join("host_lib.d.ts"), "declare const hostSecret: string;\n").unwrap();

    let from = temp.path().join("from");
    let to = temp.path().join("to");
    fs::create_dir_all(&from).unwrap();
    fs::create_dir_all(&to).unwrap();
    fs::write(from.join("package.json"), "{}").unwrap();

    let from_at_types = from.join("node_modules").join("@types");
    fs::create_dir_all(&from_at_types).unwrap();

    // 1. Malicious symlink pointing to an out-of-root host directory
    let outside_link = from_at_types.join("outside_pkg");
    let _ = std::os::unix::fs::symlink(&outside_dir, &outside_link);

    // 2. Malicious symlink pointing to an out-of-root host file
    let file_link = from_at_types.join("stolen.json");
    let _ = std::os::unix::fs::symlink(outside_dir.join("secret_credentials.json"), &file_link);

    // 3. Legitimate in-workspace pnpm package symlink
    let pnpm_valid = from
        .join("node_modules")
        .join(".pnpm")
        .join("@types+valid@1.0.0")
        .join("node_modules")
        .join("@types")
        .join("valid");
    fs::create_dir_all(&pnpm_valid).unwrap();
    fs::write(pnpm_valid.join("index.d.ts"), "export declare const validPkg: boolean;\n").unwrap();

    let valid_link = from_at_types.join("valid");
    let _ = std::os::unix::fs::symlink(&pnpm_valid, &valid_link);

    // Seed worktree
    let _ = seed_typescript_worktree(&from, &to).unwrap();

    // The legitimate in-root package must be present in the shared cache
    assert!(cache_dir.join("valid").join("index.d.ts").is_file());

    // Out-of-root symlinks must NEVER be dereferenced or copied into the shared cache
    assert!(!cache_dir.join("outside_pkg").exists(), "Out-of-root directory symlinks must be rejected!");
    assert!(!cache_dir.join("stolen.json").exists(), "Out-of-root file symlinks must be rejected!");
    assert!(!cache_dir.join("secret_credentials.json").exists());
    assert!(!cache_dir.join("host_lib.d.ts").exists());

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_seed_typescript_worktree_rejects_out_of_root_traversal_root() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("ts-cache-root-security");
    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &cache_dir);
    }

    // External directory simulating sensitive host files
    let outside_dir = temp.path().join("outside_sensitive_dir");
    fs::create_dir_all(&outside_dir).unwrap();
    fs::write(outside_dir.join("config.json"), "{\"secret\": \"OUTSIDE_SECRET\"}\n").unwrap();
    fs::write(outside_dir.join("types.d.ts"), "declare const sensitive: any;\n").unwrap();

    let from_evil = temp.path().join("from_evil");
    let to_evil = temp.path().join("to_evil");
    fs::create_dir_all(&from_evil).unwrap();
    fs::create_dir_all(&to_evil).unwrap();
    fs::write(from_evil.join("package.json"), "{}").unwrap();

    let evil_node_modules = from_evil.join("node_modules");
    fs::create_dir_all(&evil_node_modules).unwrap();

    // node_modules/@types ITSELF is a symlink pointing to outside_dir!
    let evil_at_types = evil_node_modules.join("@types");
    let _ = std::os::unix::fs::symlink(&outside_dir, &evil_at_types);

    // find_project_types must reject this out-of-root traversal root
    let discovered = find_project_types(&from_evil);
    assert!(discovered.is_empty(), "find_project_types must reject out-of-root traversal root symlink");

    // tree_size must return 0
    let size = tree_size(&evil_at_types);
    assert_eq!(size, 0, "tree_size must reject out-of-root traversal root symlink");

    // merge_types must return 0 and not copy anything
    let merged = merge_types(&evil_at_types, &cache_dir).unwrap();
    assert_eq!(merged, 0, "merge_types must return 0 for out-of-root traversal root");

    // seed_typescript_worktree must not copy any external files
    let _ = seed_typescript_worktree(&from_evil, &to_evil).unwrap();
    assert!(!cache_dir.join("config.json").exists());
    assert!(!cache_dir.join("types.d.ts").exists());

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_find_project_types_symlink_cycle_and_out_of_root_custom_types() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("ts-cache-cycle-test");
    unsafe {
        std::env::set_var(TS_TYPES_CACHE_ENV, &cache_dir);
    }

    let root = temp.path().join("project");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("package.json"), "{}").unwrap();

    // 1. External out-of-root directory with declaration files
    let outside_dir = temp.path().join("outside_custom");
    fs::create_dir_all(&outside_dir).unwrap();
    fs::write(outside_dir.join("global.d.ts"), "declare const outside: string;\n").unwrap();

    // A malicious custom "typings" directory symlink pointing to outside_dir
    let evil_typings = root.join("typings");
    let _ = std::os::unix::fs::symlink(&outside_dir, &evil_typings);

    // 2. A custom "types" directory containing a self/ancestor symlink cycle
    let custom_types = root.join("types");
    let sub = custom_types.join("sub");
    fs::create_dir_all(&sub).unwrap();
    // Direct self-loop: types/loop -> types
    let _ = std::os::unix::fs::symlink(&custom_types, custom_types.join("loop"));
    // Ancestor cycle: types/sub/ancestor_loop -> types
    let _ = std::os::unix::fs::symlink(&custom_types, sub.join("ancestor_loop"));

    // Case A: types directory has cycles but NO declaration files.
    // has_declaration_files must terminate without stack overflow and return false!
    assert!(!has_declaration_files(&custom_types));

    // Case B: Now add a valid declaration file inside types/
    fs::write(custom_types.join("local.d.ts"), "export declare const local: boolean;\n").unwrap();
    // has_declaration_files must terminate, not get trapped in cycle, and return true!
    assert!(has_declaration_files(&custom_types));

    // find_project_types must discover `types` (valid in-root with d.ts and cycle-safe),
    // and must strictly reject `typings` (out-of-root symlink).
    let discovered = find_project_types(&root);
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0], custom_types);

    // Seeding worktree must succeed without stack overflow and copy only local.d.ts
    let dest = temp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    let seeded = seed_typescript_worktree(&root, &dest).unwrap();
    assert!(seeded.is_some());
    assert!(cache_dir.join("types").join("local.d.ts").is_file());
    assert!(!cache_dir.join("global.d.ts").exists());

    unsafe {
        std::env::remove_var(TS_TYPES_CACHE_ENV);
    }
}

#[test]
fn test_approved_target_and_canonical_dereferencing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir_all(&root).unwrap();

    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.d.ts"), "declare const secret: string;\n").unwrap();

    let inside = root.join("valid_types");
    fs::create_dir_all(&inside).unwrap();
    fs::write(inside.join("index.d.ts"), "export declare const ok: boolean;\n").unwrap();

    let approved = build_approved_roots(&[&root]);

    // Symlink pointing to approved in-root directory
    let sym_inside = root.join("link_inside");
    let _ = std::os::unix::fs::symlink(&inside, &sym_inside);
    let target = approved_target(&sym_inside, &approved);
    assert!(target.is_some());
    assert_eq!(target.unwrap(), inside.canonicalize().unwrap());

    // Symlink pointing to outside directory
    let sym_outside = root.join("link_outside");
    let _ = std::os::unix::fs::symlink(&outside, &sym_outside);
    let target_out = approved_target(&sym_outside, &approved);
    assert!(target_out.is_none());
}
