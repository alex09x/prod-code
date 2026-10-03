#![cfg(unix)]

use prod_code_gateway::{polyglot_compiler_cache_env, DiskSpace};
use prod_code_gateway::python_cache::{
    find_venv_stubs, is_python_project, merge_stubs, prune_stale_stub_cache_in,
    python_stub_cache_dir, python_stub_cache_env, seed_python_worktree,
    seed_python_worktree_within, tree_size, PYTHON_STUB_CACHE_ENV,
};
use std::fs;
use std::time::Duration;

static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_python_stub_cache_dir_and_env_resolution() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("isolated-python-stubs");

    unsafe {
        std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom_cache);
    }

    let cache_dir = python_stub_cache_dir();
    assert_eq!(cache_dir, custom_cache);
    assert!(cache_dir.is_dir());

    let envs = python_stub_cache_env();
    let cache_str = custom_cache.to_str().unwrap();

    assert!(envs.iter().any(|(k, v)| k == PYTHON_STUB_CACHE_ENV && v == cache_str));
    assert!(envs.iter().any(|(k, v)| k == "MYPYPATH" && v == cache_str));
    assert!(envs.iter().any(|(k, v)| k == "TYPINGS_PATH" && v == cache_str));

    unsafe {
        std::env::remove_var(PYTHON_STUB_CACHE_ENV);
    }
}

#[test]
fn test_polyglot_compiler_cache_env_contains_python_stub_cache() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("polyglot-python-cache");

    unsafe {
        std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom_cache);
    }

    let ws = temp.path().join("repo");
    fs::create_dir_all(&ws).unwrap();

    let envs = polyglot_compiler_cache_env(&ws, false, None);
    let cache_str = custom_cache.to_str().unwrap();

    assert!(envs.iter().any(|(k, v)| k == PYTHON_STUB_CACHE_ENV && v == cache_str));
    assert!(envs.iter().any(|(k, v)| k == "MYPYPATH" && v == cache_str));
    assert!(envs.iter().any(|(k, v)| k == "TYPINGS_PATH" && v == cache_str));

    unsafe {
        std::env::remove_var(PYTHON_STUB_CACHE_ENV);
    }
}

#[test]
fn test_is_python_project_detection() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    assert!(!is_python_project(root));

    // pyproject.toml detection
    fs::write(root.join("pyproject.toml"), "[project]\nname=\"foo\"\n").unwrap();
    assert!(is_python_project(root));
}

#[test]
fn test_seed_python_worktree_typings_and_symlink() {
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

    // Local typings with .pyi files
    let from_typings = from.join("typings").join("pandas");
    fs::create_dir_all(&from_typings).unwrap();
    fs::write(from_typings.join("__init__.pyi"), "class DataFrame: ...\n").unwrap();
    fs::write(from_typings.join("py.typed"), "").unwrap();

    let result = seed_python_worktree(&from, &to).unwrap();
    assert!(result.is_some());

    // Verify stubs were merged into shared stub cache
    assert!(custom_cache.join("pandas").join("__init__.pyi").is_file());
    assert!(custom_cache.join("pandas").join("py.typed").is_file());

    // Verify to/typings symlink points to shared stub cache
    let to_typings = to.join("typings");
    assert!(fs::symlink_metadata(&to_typings).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_link(&to_typings).unwrap(), custom_cache);

    unsafe {
        std::env::remove_var(PYTHON_STUB_CACHE_ENV);
    }
}

#[test]
fn test_seed_python_worktree_venv_stubs_discovery() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("shared-stubs-venv");
    unsafe {
        std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom_cache);
    }

    let from = temp.path().join("origin-project");
    let to = temp.path().join("worktree-project");
    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("requirements.txt"), "torch\n").unwrap();

    // Mock virtual environment with *-stubs
    let sp = from.join(".venv").join("lib").join("python3.11").join("site-packages");
    let torch_stubs = sp.join("torch-stubs");
    fs::create_dir_all(&torch_stubs).unwrap();
    fs::write(torch_stubs.join("__init__.pyi"), "class Tensor: ...\n").unwrap();

    let found_stubs = find_venv_stubs(&from.join(".venv"));
    assert_eq!(found_stubs.len(), 1);

    let result = seed_python_worktree(&from, &to).unwrap();
    assert!(result.is_some());

    // Verify torch-stubs indexed into shared cache
    assert!(custom_cache.join("torch-stubs").join("__init__.pyi").is_file());

    unsafe {
        std::env::remove_var(PYTHON_STUB_CACHE_ENV);
    }
}

#[test]
fn test_seed_python_worktree_updates_pyrightconfig() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("shared-stubs-cfg");
    unsafe {
        std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom_cache);
    }

    let from = temp.path().join("origin-project");
    let to = temp.path().join("worktree-project");
    fs::create_dir_all(&from).unwrap();
    fs::create_dir_all(&to).unwrap();
    fs::write(from.join("setup.py"), "# setup\n").unwrap();
    fs::write(to.join("pyrightconfig.json"), r#"{"include":["src"]}"#).unwrap();

    let result = seed_python_worktree(&from, &to).unwrap();
    assert!(result.is_some());

    let cfg_str = fs::read_to_string(to.join("pyrightconfig.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&cfg_str).unwrap();
    assert_eq!(parsed.get("stubPath").and_then(|s| s.as_str()), Some("typings"));

    unsafe {
        std::env::remove_var(PYTHON_STUB_CACHE_ENV);
    }
}

#[test]
fn test_seed_python_worktree_skips_non_python_projects() {
    let temp = tempfile::tempdir().unwrap();
    let from = temp.path().join("rust-workspace");
    let to = temp.path().join("new-worktree");

    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("Cargo.toml"), "[package]\nname=\"foo\"\n").unwrap();

    let result = seed_python_worktree(&from, &to).unwrap();
    assert!(result.is_none());
    assert!(!to.exists());
}

#[test]
fn test_seed_python_worktree_skips_when_disk_space_insufficient() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("shared-stubs-tight");
    unsafe {
        std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom_cache);
    }

    let from = temp.path().join("origin-project");
    let to = temp.path().join("worktree-project");
    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("pyproject.toml"), "[project]\nname=\"large\"\n").unwrap();

    let from_typings = from.join("typings").join("heavy_module");
    fs::create_dir_all(&from_typings).unwrap();
    fs::write(from_typings.join("stubs.pyi"), vec![b'P'; 100_000]).unwrap();

    let restricted_space = Some(DiskSpace {
        free: 100,
        total: 1_000_000,
    });

    let result = seed_python_worktree_within(&from, &to, restricted_space).unwrap();
    assert!(result.is_some());

    // Heavy stubs not copied due to insufficient space
    assert!(!custom_cache.join("heavy_module").exists());

    // But symlink to typings still established
    let to_typings = to.join("typings");
    assert!(fs::symlink_metadata(&to_typings).unwrap().file_type().is_symlink());

    unsafe {
        std::env::remove_var(PYTHON_STUB_CACHE_ENV);
    }
}

#[test]
fn test_prune_stale_stub_cache_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    fs::create_dir_all(&cache_dir).unwrap();

    let stub1 = cache_dir.join("stub1.pyi");
    let stub2 = cache_dir.join("stub2.pyi");
    let stub3 = cache_dir.join("stub3.pyi");
    let lock_file = cache_dir.join("stub1.pyi.lock");

    fs::write(&stub1, vec![1u8; 1000]).unwrap();
    fs::write(&stub2, vec![2u8; 1000]).unwrap();
    fs::write(&stub3, vec![3u8; 1000]).unwrap();
    fs::write(&lock_file, b"lock content").unwrap();

    // Prune with max_size_bytes 2500 should evict 1 stub and preserve lock file
    let evicted = prune_stale_stub_cache_in(&cache_dir, Duration::from_secs(3600), 2500).unwrap();
    assert_eq!(evicted, 1);
    assert!(lock_file.is_file(), "lock files must never be pruned");
}

#[test]
fn test_prune_stale_stub_cache_ignores_symlinks_and_external_targets() {
    let temp = tempfile::tempdir().unwrap();
    let cache_dir = temp.path().join("cache");
    let external_dir = temp.path().join("external-dir");
    fs::create_dir_all(&cache_dir).unwrap();
    fs::create_dir_all(&external_dir).unwrap();

    let external_file = external_dir.join("critical.pyi");
    fs::write(&external_file, b"do not delete me!").unwrap();

    let symlinked_dir = cache_dir.join("symlink_to_external");
    std::os::unix::fs::symlink(&external_dir, &symlinked_dir).unwrap();

    let regular_file = cache_dir.join("regular.pyi");
    fs::write(&regular_file, vec![1u8; 5000]).unwrap();

    let evicted = prune_stale_stub_cache_in(&cache_dir, Duration::from_secs(3600), 100).unwrap();
    assert_eq!(evicted, 1);
    assert!(!regular_file.exists());

    // External file must remain intact, symlink not traversed or deleted
    assert!(external_file.is_file());
    assert!(symlinked_dir.exists());
}

#[test]
fn test_prune_stale_stub_cache_rejects_root_file_or_symlink_swap() {
    let temp = tempfile::tempdir().unwrap();
    let regular_file = temp.path().join("regular_file.pyi");
    fs::write(&regular_file, b"content").unwrap();

    let evicted = prune_stale_stub_cache_in(&regular_file, Duration::from_secs(3600), 100).unwrap();
    assert_eq!(evicted, 0);
    assert!(regular_file.is_file());
}

#[test]
fn test_merge_stubs_direct() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    fs::create_dir_all(&src).unwrap();

    fs::write(src.join("module.pyi"), b"def foo() -> int: ...\n").unwrap();
    let written = merge_stubs(&src, &dst).unwrap();
    assert!(written > 0);
    assert!(dst.join("module.pyi").is_file());
}

#[test]
fn test_seed_python_worktree_skips_venv_stubs_when_disk_space_insufficient() {
    let _guard = TEST_ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let custom_cache = temp.path().join("shared-stubs-venv-tight");
    unsafe {
        std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom_cache);
    }

    let from = temp.path().join("origin-project");
    let to = temp.path().join("worktree-project");
    fs::create_dir_all(&from).unwrap();
    fs::write(from.join("pyproject.toml"), "[project]\nname=\"large\"\n").unwrap();

    let venv_stubs = from
        .join(".venv")
        .join("lib")
        .join("python3.11")
        .join("site-packages")
        .join("pandas-stubs");
    fs::create_dir_all(&venv_stubs).unwrap();
    fs::write(venv_stubs.join("core.pyi"), vec![b'D'; 100_000]).unwrap();

    let restricted_space = Some(DiskSpace {
        free: 100,
        total: 1_000_000,
    });

    let result = seed_python_worktree_within(&from, &to, restricted_space).unwrap();
    assert!(result.is_some());

    // venv stubs must NOT be copied due to insufficient space
    assert!(!custom_cache.join("pandas-stubs").exists());

    // But symlink to typings still established
    let to_typings = to.join("typings");
    assert!(fs::symlink_metadata(&to_typings).unwrap().file_type().is_symlink());

    unsafe {
        std::env::remove_var(PYTHON_STUB_CACHE_ENV);
    }
}

#[test]
fn test_merge_stubs_atomic_publishing() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    fs::create_dir_all(&src).unwrap();

    // Initial version
    fs::write(src.join("types.pyi"), b"x: int = 1\n").unwrap();
    let written = merge_stubs(&src, &dst).unwrap();
    assert!(written > 0);
    assert_eq!(fs::read(dst.join("types.pyi")).unwrap(), b"x: int = 1\n");

    // Updated version
    std::thread::sleep(Duration::from_millis(50));
    fs::write(src.join("types.pyi"), b"x: int = 2\n").unwrap();
    let updated = merge_stubs(&src, &dst).unwrap();
    assert!(updated > 0);
    assert_eq!(fs::read(dst.join("types.pyi")).unwrap(), b"x: int = 2\n");

    // No leftover temporary files
    for entry in fs::read_dir(&dst).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        assert!(!name.starts_with(".tmp-stub-"), "leftover tmp file: {name}");
    }
}

#[test]
fn test_merge_stubs_concurrent_newer_wins_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let src_old = temp.path().join("src_old");
    let src_new = temp.path().join("src_new");
    let dst = temp.path().join("dst");
    fs::create_dir_all(&src_old).unwrap();
    fs::create_dir_all(&src_new).unwrap();

    // Older version
    fs::write(src_old.join("version.pyi"), b"VERSION = '1.0.0'\n").unwrap();

    // Newer version with later mtime
    std::thread::sleep(Duration::from_millis(50));
    fs::write(src_new.join("version.pyi"), b"VERSION = '2.0.0'\n").unwrap();

    // 1. Publish newer version first
    let written = merge_stubs(&src_new, &dst).unwrap();
    assert!(written > 0);
    assert_eq!(fs::read(dst.join("version.pyi")).unwrap(), b"VERSION = '2.0.0'\n");

    // 2. An older source merges later (e.g. slow worker racing)
    let re_written = merge_stubs(&src_old, &dst).unwrap();
    assert_eq!(re_written, 0);

    // Newer version MUST NOT be overwritten by the older version
    assert_eq!(fs::read(dst.join("version.pyi")).unwrap(), b"VERSION = '2.0.0'\n");
}

#[test]
fn test_tree_size_and_merge_stubs_handle_symlink_cycle() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    fs::create_dir_all(&src).unwrap();

    let sub = src.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("types.pyi"), b"x: int = 10\n").unwrap();

    // Create circular directory symlinks pointing to parent and self
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&src, sub.join("cycle_to_parent")).unwrap();
        std::os::unix::fs::symlink(&sub, sub.join("cycle_to_self")).unwrap();
    }

    // tree_size must terminate safely without stack overflow and report correct file size
    let size = tree_size(&src);
    assert_eq!(size, b"x: int = 10\n".len() as u64);

    // merge_stubs must terminate safely without stack overflow, copying only real stubs
    let merged = merge_stubs(&src, &dst).unwrap();
    assert!(merged > 0);
    assert_eq!(fs::read(dst.join("sub").join("types.pyi")).unwrap(), b"x: int = 10\n");
    assert!(!dst.join("sub").join("cycle_to_parent").exists());
    assert!(!dst.join("sub").join("cycle_to_self").exists());
}
