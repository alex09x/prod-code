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
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(unix)]
pub(super) fn make_executable_script(path: &Path, body: &str) {
    let script = format!("#!/bin/sh\n{body}\n");
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn probe_success_extracts_valid_directory() {
    let temp = tempfile::tempdir().unwrap();
    let fake_root = temp.path().join("global_node_modules");
    std::fs::create_dir_all(&fake_root).unwrap();

    let fake_npm = temp.path().join("fake_npm");
    make_executable_script(
        &fake_npm,
        &format!("echo \"{}\"", fake_root.to_string_lossy()),
    );

    let result = probe_npm_root(&fake_npm, Duration::from_secs(2), []);
    assert_eq!(result, Some(fake_root));
}

#[cfg(unix)]
#[test]
fn probe_suppresses_persistent_logging_args_and_env() {
    let temp = tempfile::tempdir().unwrap();
    let fake_root = temp.path().join("global_modules");
    std::fs::create_dir_all(&fake_root).unwrap();

    let log_file = temp.path().join("args_log.txt");
    let fake_npm = temp.path().join("fake_npm_logging");
    make_executable_script(
        &fake_npm,
        &format!(
            "for a in \"$@\"; do echo \"$a\" >> \"{}\"; done\n\
             echo \"LOGS_MAX=$npm_config_logs_max\" >> \"{}\"\n\
             echo \"NOTIFIER=$npm_config_update_notifier\" >> \"{}\"\n\
             echo \"{}\"",
            log_file.to_string_lossy(),
            log_file.to_string_lossy(),
            log_file.to_string_lossy(),
            fake_root.to_string_lossy(),
        ),
    );

    let result = probe_npm_root(&fake_npm, Duration::from_secs(2), []);
    assert_eq!(result, Some(fake_root));

    let logged = std::fs::read_to_string(&log_file).unwrap();
    assert!(logged.contains("root"));
    assert!(logged.contains("-g"));
    assert!(logged.contains("--logs-max=0"));
    assert!(logged.contains("--no-update-notifier"));
    assert!(logged.contains("LOGS_MAX=0"));
    assert!(logged.contains("NOTIFIER=false"));
}

#[cfg(unix)]
#[test]
fn probe_failure_on_nonzero_exit() {
    let temp = tempfile::tempdir().unwrap();
    let fake_root = temp.path().join("modules");
    std::fs::create_dir_all(&fake_root).unwrap();

    let fake_npm = temp.path().join("failing_npm");
    make_executable_script(
        &fake_npm,
        &format!("echo \"{}\"\nexit 1", fake_root.to_string_lossy()),
    );

    let result = probe_npm_root(&fake_npm, Duration::from_secs(2), []);
    assert_eq!(
        result, None,
        "non-zero exit code must not be treated as success"
    );
}

#[cfg(unix)]
#[test]
fn probe_failure_on_nonexistent_directory() {
    let temp = tempfile::tempdir().unwrap();
    let non_existent = temp.path().join("does_not_exist_xyz");

    let fake_npm = temp.path().join("missing_dir_npm");
    make_executable_script(
        &fake_npm,
        &format!("echo \"{}\"", non_existent.to_string_lossy()),
    );

    let result = probe_npm_root(&fake_npm, Duration::from_secs(2), []);
    assert_eq!(
        result, None,
        "non-existent path must not be treated as a valid root"
    );
}

#[cfg(unix)]
#[test]
fn probe_failure_on_empty_output() {
    let temp = tempfile::tempdir().unwrap();
    let fake_npm = temp.path().join("empty_npm");
    make_executable_script(&fake_npm, "echo \"   \"");

    let result = probe_npm_root(&fake_npm, Duration::from_secs(2), []);
    assert_eq!(
        result, None,
        "empty output must not be treated as a valid root"
    );
}

#[cfg(unix)]
#[test]
fn probe_timeout_reaps_child_process() {
    let temp = tempfile::tempdir().unwrap();
    let fake_npm = temp.path().join("hanging_npm");
    make_executable_script(&fake_npm, "sleep 10");

    let start = Instant::now();
    let result = probe_npm_root(&fake_npm, Duration::from_millis(100), []);
    let elapsed = start.elapsed();

    assert_eq!(result, None, "timed-out probe must return None");
    assert!(
        elapsed < Duration::from_secs(2),
        "timeout must abort promptly, took {elapsed:?}"
    );
}

#[cfg(unix)]
#[test]
fn caching_success_and_negative_caching_failure() {
    let temp = tempfile::tempdir().unwrap();
    let fake_root = temp.path().join("node_modules");
    std::fs::create_dir_all(&fake_root).unwrap();

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let fake_npm = temp.path().join("counted_npm");
    make_executable_script(
        &fake_npm,
        &format!("echo \"{}\"", fake_root.to_string_lossy()),
    );

    let cache = NpmRootCache::new();

    // 1. Success caching
    let lookup = || {
        counter_clone.fetch_add(1, Ordering::SeqCst);
        Some(fake_npm.clone())
    };
    let env_lookup = |_key: &str| None;

    let res1 = npm_global_root_with(&cache, lookup, env_lookup, Duration::from_secs(2));
    assert_eq!(res1, Some(fake_root.clone()));
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    let res2 = npm_global_root_with(&cache, lookup, env_lookup, Duration::from_secs(2));
    assert_eq!(res2, Some(fake_root.clone()));
    // lookup was called to check exe existence, but probe was answered from fresh cache
    let entry = cache.cached_entry().expect("cache entry must exist");
    assert_eq!(entry.result, Some(fake_root.clone()));

    // 2. Negative caching
    let fail_cache = NpmRootCache::new();
    let fail_counter = Arc::new(AtomicUsize::new(0));
    let fail_counter_clone = fail_counter.clone();

    let missing_lookup = || {
        fail_counter_clone.fetch_add(1, Ordering::SeqCst);
        None
    };

    let f1 = npm_global_root_with(
        &fail_cache,
        missing_lookup,
        env_lookup,
        Duration::from_secs(2),
    );
    assert_eq!(f1, None);

    let f2 = npm_global_root_with(
        &fail_cache,
        missing_lookup,
        env_lookup,
        Duration::from_secs(2),
    );
    assert_eq!(f2, None);

    let fail_entry = fail_cache.cached_entry().expect("failure must be cached");
    assert_eq!(fail_entry.result, None);
}

#[cfg(unix)]
#[test]
fn revalidation_when_executable_lookup_changes() {
    let temp = tempfile::tempdir().unwrap();
    let fake_root = temp.path().join("installed_modules");
    std::fs::create_dir_all(&fake_root).unwrap();

    let fake_npm = temp.path().join("late_npm");
    make_executable_script(
        &fake_npm,
        &format!("echo \"{}\"", fake_root.to_string_lossy()),
    );

    let cache = NpmRootCache::new();
    let installed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let installed_clone = installed.clone();
    let fake_npm_clone = fake_npm.clone();

    let lookup = move || {
        if installed_clone.load(Ordering::SeqCst) {
            Some(fake_npm_clone.clone())
        } else {
            None
        }
    };
    let env_lookup = |_key: &str| None;

    // First call: npm not installed
    let first = npm_global_root_with(&cache, &lookup, env_lookup, Duration::from_secs(2));
    assert_eq!(first, None);

    // npm gets installed
    installed.store(true, Ordering::SeqCst);

    // Second call: executable lookup change triggers immediate revalidation
    let second = npm_global_root_with(&cache, &lookup, env_lookup, Duration::from_secs(2));
    assert_eq!(second, Some(fake_root));
}

#[cfg(unix)]
#[test]
fn revalidation_when_environment_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root_a = temp.path().join("modules_a");
    let root_b = temp.path().join("modules_b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();

    let fake_npm = temp.path().join("env_aware_npm");
    make_executable_script(
        &fake_npm,
        &format!(
            "if [ \"$PREFIX\" = \"prefix_b\" ]; then echo \"{}\"; else echo \"{}\"; fi",
            root_b.to_string_lossy(),
            root_a.to_string_lossy(),
        ),
    );

    let cache = NpmRootCache::new();
    let prefix = Arc::new(std::sync::RwLock::new("prefix_a".to_string()));
    let prefix_clone = prefix.clone();

    let lookup = || Some(fake_npm.clone());
    let env_lookup = move |key: &str| {
        if key == "PREFIX" {
            Some(OsString::from(prefix_clone.read().unwrap().clone()))
        } else {
            None
        }
    };

    let first = npm_global_root_with(&cache, lookup, &env_lookup, Duration::from_secs(2));
    assert_eq!(first, Some(root_a));

    // Change PREFIX environment
    *prefix.write().unwrap() = "prefix_b".to_string();

    // Environment change triggers revalidation
    let second = npm_global_root_with(&cache, lookup, &env_lookup, Duration::from_secs(2));
    assert_eq!(second, Some(root_b));
}

#[cfg(unix)]
#[test]
fn revalidation_when_cached_directory_is_removed() {
    let temp = tempfile::tempdir().unwrap();
    let dynamic_root = temp.path().join("removable_modules");
    std::fs::create_dir_all(&dynamic_root).unwrap();

    let fake_npm = temp.path().join("dir_npm");
    make_executable_script(
        &fake_npm,
        &format!("echo \"{}\"", dynamic_root.to_string_lossy()),
    );

    let cache = NpmRootCache::new();
    let lookup = || Some(fake_npm.clone());
    let env_lookup = |_key: &str| None;

    let res = npm_global_root_with(&cache, lookup, env_lookup, Duration::from_secs(2));
    assert_eq!(res, Some(dynamic_root.clone()));

    // Directory gets removed
    std::fs::remove_dir(&dynamic_root).unwrap();

    // Next lookup notices directory is gone and revalidates (now finding it missing)
    let after_removal = npm_global_root_with(&cache, lookup, env_lookup, Duration::from_secs(2));
    assert_eq!(after_removal, None);
}

#[test]
fn native_typescript_lsp_with_fake_structure() {
    let temp = tempfile::tempdir().unwrap();
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };

    let tsc_dir = temp
        .path()
        .join("typescript")
        .join("node_modules")
        .join("@typescript")
        .join(format!("typescript-{os}-{arch}"))
        .join("lib");
    std::fs::create_dir_all(&tsc_dir).unwrap();
    let fake_tsc = tsc_dir.join("tsc");
    std::fs::write(&fake_tsc, "").unwrap();

    let found = native_typescript_lsp_with(None, Some(temp.path()));
    assert_eq!(found, Some(fake_tsc));

    let not_found = native_typescript_lsp_with(None, None);
    assert_eq!(not_found, None);
}

#[test]
fn resolve_npm_binary_from_path_variable() {
    let temp = tempfile::tempdir().unwrap();
    let fake_bin = temp.path().join("npm");
    // Executable: on Unix a file without an execute bit is skipped.
    #[cfg(unix)]
    make_executable_script(&fake_bin, "exit 0");
    #[cfg(not(unix))]
    std::fs::write(&fake_bin, "").unwrap();

    let env_lookup = |key: &str| {
        if key == "PATH" {
            Some(temp.path().as_os_str().to_os_string())
        } else {
            None
        }
    };

    let resolved = resolve_npm_binary_with(env_lookup);
    assert_eq!(resolved, Some(fake_bin));
}

mod cache_and_path;
mod descendant_pipe;
#[cfg(unix)]
mod log_dir;
#[cfg(unix)]
mod ownership;
