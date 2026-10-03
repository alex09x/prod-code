#![cfg(unix)]

use prod_code_gateway::{polyglot_compiler_cache_env, DiskSpace};
use prod_code_gateway::ts_cache::{
    find_project_types, is_typescript_project, merge_types, prune_stale_types_cache_in,
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

    // Verify types are copied into shared cache
    let cached_react = custom_cache.join("react").join("index.d.ts");
    assert!(cached_react.is_file());
    assert!(fs::read_to_string(&cached_react).unwrap().contains("useState"));

    // Verify to/node_modules/@types is a symlink pointing to custom_cache
    let to_at_types = to.join("node_modules").join("@types");
    let meta = fs::symlink_metadata(&to_at_types).unwrap();
    assert!(meta.file_type().is_symlink());
    let target = fs::read_link(&to_at_types).unwrap();
    assert_eq!(target, custom_cache);

    // Verify to sees the cached declaration
    let to_react_dts = to_at_types.join("react").join("index.d.ts");
    assert!(to_react_dts.is_file());
    assert!(fs::read_to_string(&to_react_dts).unwrap().contains("useState"));

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

    // Verify it was merged into custom_cache and replaced with a symlink to deduplicate!
    let meta = fs::symlink_metadata(&to_at_types).unwrap();
    assert!(meta.file_type().is_symlink());
    assert!(custom_cache.join("express").join("index.d.ts").is_file());

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
fn test_coordinate_tsconfig_typeroots() {
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
    assert!(type_roots.iter().any(|v| v.as_str() == Some("node_modules/@types")));
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
