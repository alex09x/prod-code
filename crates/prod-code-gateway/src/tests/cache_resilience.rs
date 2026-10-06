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
use prod_code_protocol::FileDelta;
use std::path::PathBuf;

#[tokio::test]
async fn refresh_engines_safely_handles_unloaded_or_failing_updates() {
    let storage = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let ws_dir = storage.path().join("ws");
    std::fs::create_dir_all(&ws_dir).unwrap();
    let files = vec![FileDelta {
        relative_path: "src/lib.rs".to_string(),
        content: Some(b"pub fn dummy() {}\n".to_vec()),
        is_executable: false,
    }];
    refresh_engines(&manager, &ws_dir, &files).await;
}

#[test]
fn test_effective_prune_timeouts() {
    let cli = ServerCli {
        bind: "0.0.0.0:9400".parse().unwrap(),
        socket_path: None,
        storage: PathBuf::from("/tmp/storage"),
        idle_evict_secs: 1800,
        engine_reserve_mib: 0,
        max_concurrent_engine_loads: 0,
        prune_worktree_secs: 3600,
        prune_worktree_days: None,
        prune_workspace_secs: 86400,
        prune_workspace_days: None,
        prune_below_free_percent: 15,
        engines: vec![],
        shadow_dir: None,
        peers: String::new(),
        advertise: None,
        build_cache_ram: false,
        build_cache_dir: None,
    };
    // Defaults: 1 hour (3600s) for worktrees, 24 hours (86400s) for main workspaces
    assert_eq!(cli.effective_prune_worktree_secs(), 3600);
    assert_eq!(cli.effective_prune_workspace_secs(), 86400);

    // Days overrides
    let mut cli_days = cli.clone();
    cli_days.prune_worktree_days = Some(7);
    cli_days.prune_workspace_days = Some(3);
    assert_eq!(cli_days.effective_prune_worktree_secs(), 7 * 86_400);
    assert_eq!(cli_days.effective_prune_workspace_secs(), 3 * 86_400);

    // Disabling with 0
    let mut cli_disabled = cli.clone();
    cli_disabled.prune_worktree_secs = 0;
    cli_disabled.prune_workspace_days = Some(0);
    assert_eq!(cli_disabled.effective_prune_worktree_secs(), 0);
    assert_eq!(cli_disabled.effective_prune_workspace_secs(), 0);
}

#[test]
fn test_prewarm_virtualenv_pycache_safe_on_missing_or_invalid() {
    let temp = tempfile::tempdir().unwrap();
    let empty_venv = temp.path().join("empty_venv");
    std::fs::create_dir_all(&empty_venv).unwrap();
    // Missing python binary -> Ok(0)
    let res = prewarm_virtualenv_pycache(&empty_venv);
    assert_eq!(res.unwrap(), 0);

    // Invalid non-executable python file -> Ok(0) without crashing
    let bin_dir = empty_venv.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let py_file = bin_dir.join("python");
    std::fs::write(&py_file, b"not executable").unwrap();
    let res = prewarm_virtualenv_pycache(&empty_venv);
    assert_eq!(res.unwrap(), 0);
}

#[test]
fn test_polyglot_compiler_cache_env_composition() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path().join("my-project");
    std::fs::create_dir_all(&ws).unwrap();

    let ram_target = temp.path().join("ram-target");
    let envs = polyglot_compiler_cache_env(&ws, false, Some(&ram_target));
    assert!(
        envs.iter()
            .any(|(k, v)| k == "CARGO_TARGET_DIR" && v == ram_target.to_str().unwrap())
    );
    assert!(
        envs.iter()
            .any(|(k, _)| k == "SWIFTPM_MODULECACHE_OVERRIDE")
    );
    assert!(envs.iter().any(|(k, _)| k == "SWIFT_MODULE_CACHE_PATH"));
    assert!(envs.iter().any(|(k, _)| k == "CLANG_MODULE_CACHE_PATH"));

    let ccache_envs = polyglot_compiler_cache_env(&ws, true, None);
    assert!(
        ccache_envs
            .iter()
            .any(|(k, v)| k == "CCACHE_BASEDIR" && v == ws.to_str().unwrap())
    );
    assert!(
        ccache_envs
            .iter()
            .any(|(k, v)| k == "CCACHE_NOHASHDIR" && v == "1")
    );
}

#[test]
fn test_resolve_ram_build_cache_workspace_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let ws1 = temp.path().join("repo-a");
    let ws2 = temp.path().join("repo-b");
    std::fs::create_dir_all(&ws1).unwrap();
    std::fs::create_dir_all(&ws2).unwrap();

    let custom_ram = temp.path().join("custom-shm");
    // Disabled returns None
    assert!(resolve_ram_build_cache(&ws1, false, Some(&custom_ram)).is_none());

    // Enabled creates isolated target directories
    let target1 = resolve_ram_build_cache(&ws1, true, Some(&custom_ram)).expect("target1");
    let target2 = resolve_ram_build_cache(&ws2, true, Some(&custom_ram)).expect("target2");

    assert!(target1.exists());
    assert!(target2.exists());
    assert_ne!(
        target1, target2,
        "workspaces must receive isolated RAM target directories"
    );
    assert!(target1.ends_with("target"));
    assert!(target2.ends_with("target"));
    assert!(target1.parent().unwrap().join(".last_used").exists());
    assert!(target2.parent().unwrap().join(".last_used").exists());
}

#[test]
fn test_sweep_ram_build_caches_removes_old_dirs() {
    let temp = tempfile::tempdir().unwrap();
    let ram_base = temp.path().join("shm");
    let ws_dir = ram_base.join("ws-old");
    let target_dir = ws_dir.join("target");
    std::fs::create_dir_all(&target_dir).unwrap();

    // Fresh dir is not swept (not older than 24h)
    let swept = sweep_ram_build_caches(&ram_base);
    assert_eq!(swept, 0);
    assert!(ws_dir.exists());

    // Active lease prevents sweeping and is recognized as active
    let lease = RamBuildLease::acquire(&target_dir);
    assert!(lease.marker.is_some());
    let marker_path = lease.marker.clone().unwrap();
    assert!(marker_path.exists());
    assert!(is_ram_lease_active(&marker_path));
    let swept_active = sweep_ram_build_caches(&ram_base);
    assert_eq!(swept_active, 0);
    assert!(ws_dir.exists());
    assert!(marker_path.exists());

    // Dropping lease unlinks its marker file
    drop(lease);
    assert!(!marker_path.exists());

    // Stale lease marker (left by crashed process) is detected as inactive and cleaned up
    let stale_lease = ws_dir.join(".active_99999_1");
    std::fs::write(&stale_lease, b"stale").unwrap();
    assert!(stale_lease.exists());
    assert!(!is_ram_lease_active(&stale_lease));
    assert!(
        !stale_lease.exists(),
        "stale lease marker must be removed when unowned"
    );

    // Old dir (>24h) with stale lease marker is swept cleanly without being permanently protected
    let stale_old = ws_dir.join(".active_88888_2");
    std::fs::write(&stale_old, b"stale-old").unwrap();
    let last_used = ws_dir.join(".last_used");
    let f = std::fs::File::create(&last_used).unwrap();
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(100_000);
    f.set_modified(past).unwrap();
    drop(f);

    let swept_stale_old = sweep_ram_build_caches(&ram_base);
    assert_eq!(swept_stale_old, 1);
    assert!(
        !ws_dir.exists(),
        "stale crash-left marker must not permanently protect cache directory"
    );

    // Non-existent base dir returns 0 safely
    let swept_none = sweep_ram_build_caches(&temp.path().join("does_not_exist"));
    assert_eq!(swept_none, 0);
}

#[test]
fn test_prewarm_virtualenv_pycache_does_not_execute_workspace_binary() {
    let temp = tempfile::tempdir().unwrap();
    let malicious_venv = temp.path().join("malicious_venv");
    let bin_dir = malicious_venv.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let canary = temp.path().join("canary_executed.txt");
    let fake_python = bin_dir.join("python");
    // Script that would create the canary file if executed
    std::fs::write(
        &fake_python,
        format!("#!/bin/sh\ntouch {}\n", canary.display()),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_python, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let _ = prewarm_virtualenv_pycache(&malicious_venv);
    assert!(
        !canary.exists(),
        "prewarm_virtualenv_pycache must never execute untrusted workspace python binary"
    );
}

#[test]
fn test_resolve_ram_build_cache_false_env_does_not_enable() {
    assert!(!is_ram_cache_enabled_with(false, Some("false")));
    assert!(!is_ram_cache_enabled_with(false, Some("0")));
    assert!(!is_ram_cache_enabled_with(false, Some("no")));
    assert!(!is_ram_cache_enabled_with(false, Some("off")));
    assert!(!is_ram_cache_enabled_with(false, None));

    assert!(is_ram_cache_enabled_with(false, Some("true")));
    assert!(is_ram_cache_enabled_with(false, Some("1")));
    assert!(is_ram_cache_enabled_with(false, Some("yes")));
    assert!(is_ram_cache_enabled_with(false, Some("on")));
    assert!(is_ram_cache_enabled_with(true, Some("false")));
    assert!(is_ram_cache_enabled_with(true, None));
}
