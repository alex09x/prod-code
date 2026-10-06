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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn test_farm_allocation_and_capacity_bounds() {
    let farm = Arc::new(ProcMacroWorkerFarm::new(8));
    assert_eq!(farm.capacity(), 8);
    assert_eq!(farm.active_workers(), 0);
    assert_eq!(farm.active_workspaces(), 0);

    // Workspace 1 asks for 6, capped at fair share (8 / 2 = 4)
    let ws1 = PathBuf::from("/tmp/ws1");
    let (w1, permit1) = farm.allocate_workers(&ws1, 6);
    assert_eq!(w1, 2, "Must cap single workspace to fair share");
    assert_eq!(permit1.worker_count(), 2);
    assert_eq!(farm.active_workers(), 2);
    assert_eq!(farm.active_workspaces(), 1);

    // Next workspaces ask for workers, each capped at fair share 2
    let ws2 = PathBuf::from("/tmp/ws2");
    let (w2, permit2) = farm.allocate_workers(&ws2, 6);
    assert_eq!(w2, 2);
    assert_eq!(farm.active_workers(), 4);
    assert_eq!(farm.active_workspaces(), 2);

    let ws3 = PathBuf::from("/tmp/ws3");
    let (w3, permit3) = farm.allocate_workers(&ws3, 4);
    assert_eq!(w3, 2);
    assert_eq!(farm.active_workers(), 6);
    assert_eq!(farm.active_workspaces(), 3);

    let ws4 = PathBuf::from("/tmp/ws4");
    let (w4, permit4) = farm.allocate_workers(&ws4, 2);
    assert_eq!(w4, 2);
    assert_eq!(farm.active_workers(), 8);
    assert_eq!(farm.active_workspaces(), 4);

    // Fifth workspace asks for workers when capacity is full:
    // Must be rejected with 0 workers (no overcommit / phantom rebalancing)
    let ws5 = PathBuf::from("/tmp/ws5");
    let (w5, permit5) = farm.allocate_workers(&ws5, 2);
    assert_eq!(w5, 0, "Must be rejected when farm is at full capacity");
    assert_eq!(permit5.worker_count(), 0);
    assert_eq!(
        farm.active_workers(),
        8,
        "Total active workers must not exceed capacity"
    );
    assert_eq!(farm.active_workspaces(), 4);

    // Dropping ws1 frees its 2 workers
    drop(permit1);
    assert_eq!(farm.active_workers(), 6);
    assert_eq!(farm.active_workspaces(), 3);

    // Now ws5 can allocate the freed workers
    let (w5_retry, permit5_retry) = farm.allocate_workers(&ws5, 2);
    assert_eq!(w5_retry, 2);
    assert_eq!(farm.active_workers(), 8);
    assert_eq!(farm.active_workspaces(), 4);

    drop(permit2);
    drop(permit3);
    drop(permit4);
    drop(permit5);
    drop(permit5_retry);
    assert_eq!(farm.active_workers(), 0);
    assert_eq!(farm.active_workspaces(), 0);
}

#[test]
fn test_farm_timeout_queued_allocation() {
    let farm = Arc::new(ProcMacroWorkerFarm::new(2));
    let ws1 = PathBuf::from("/tmp/ws_queue_1");
    let (w1, permit1) = farm.allocate_workers(&ws1, 1);
    assert_eq!(w1, 1);
    let ws2 = PathBuf::from("/tmp/ws_queue_2");
    let (w2, permit2) = farm.allocate_workers(&ws2, 1);
    assert_eq!(w2, 1);
    assert_eq!(farm.active_workers(), 2);

    let farm_clone = Arc::clone(&farm);
    let handle = std::thread::spawn(move || {
        let ws3 = PathBuf::from("/tmp/ws_queue_3");
        farm_clone.allocate_workers_timeout(&ws3, 1, Duration::from_millis(500))
    });

    // Small delay to ensure ws3 is waiting on the condvar
    std::thread::sleep(Duration::from_millis(50));
    drop(permit1);

    let (w3, permit3) = handle.join().expect("thread join failed");
    assert_eq!(
        w3, 1,
        "Queued allocation must succeed after permits released"
    );
    assert_eq!(permit3.worker_count(), 1);
    assert_eq!(farm.active_workers(), 2);
    drop(permit2);
    drop(permit3);
    assert_eq!(farm.active_workers(), 0);
}

#[test]
fn test_farm_timeout_queued_allocation_expiry() {
    let farm = Arc::new(ProcMacroWorkerFarm::new(2));
    let ws1 = PathBuf::from("/tmp/ws_exp_1");
    let (w1, permit1) = farm.allocate_workers(&ws1, 1);
    assert_eq!(w1, 1);
    let ws2 = PathBuf::from("/tmp/ws_exp_2");
    let (w2, permit2) = farm.allocate_workers(&ws2, 1);
    assert_eq!(w2, 1);
    assert_eq!(farm.active_workers(), 2);

    let ws3 = PathBuf::from("/tmp/ws_exp_3");
    let (w3, permit3) = farm.allocate_workers_timeout(&ws3, 1, Duration::from_millis(50));
    assert_eq!(w3, 0, "Must return 0 workers when wait timeout expires");
    assert_eq!(permit3.worker_count(), 0);
    assert_eq!(farm.active_workers(), 2);

    drop(permit1);
    drop(permit2);
    drop(permit3);
    assert_eq!(farm.active_workers(), 0);
}

#[test]
fn test_farm_zero_worker_allocation() {
    let farm = Arc::new(ProcMacroWorkerFarm::new(4));
    let ws = PathBuf::from("/tmp/ws_zero");
    let (w, permit) = farm.allocate_workers(&ws, 0);
    assert_eq!(w, 0);
    assert_eq!(permit.worker_count(), 0);
    assert_eq!(farm.active_workers(), 0);
    assert_eq!(farm.active_workspaces(), 0);
    drop(permit);
    assert_eq!(farm.active_workers(), 0);
}

#[test]
fn test_farm_metrics_snapshot() {
    let farm = Arc::new(ProcMacroWorkerFarm::new(16));
    let ws = PathBuf::from("/tmp/ws_metrics");
    let (_w, permit) = farm.allocate_workers(&ws, 4);
    let m = farm.metrics();
    assert_eq!(m.capacity, 16);
    assert_eq!(m.active_workers, 4);
    assert_eq!(m.active_workspaces, 1);
    assert_eq!(m.default_memory_limit_mb, 2048);
    drop(permit);
}

#[cfg(unix)]
#[test]
fn test_prepare_sandboxed_srv_execution_and_secret_scrubbing() {
    let temp = tempfile::tempdir().unwrap();
    let fake_srv = temp.path().join("fake-proc-macro-srv.sh");
    std::fs::write(
        &fake_srv,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "fake-proc-macro-srv 1.0.0"
    exit 0
fi
echo "AUTH_TOKEN='$PROD_CODE_AUTH_TOKEN'"
echo "AUTH_FILE='$PROD_CODE_AUTH_TOKEN_FILE'"
echo "SECRET_TOKEN='$PROD_CODE_TOKEN'"
echo "INTERNAL='$RUST_ANALYZER_INTERNALS_DO_NOT_USE'"
echo "TMP='$TMPDIR'"
"#,
    )
    .unwrap();

    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&fake_srv).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_srv, perms).unwrap();

    let wrapper = prepare_sandboxed_srv(&fake_srv, 1024).unwrap();
    assert!(wrapper.exists());

    // Test --version invocation
    let out = std::process::Command::new(&wrapper)
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let version_str = String::from_utf8_lossy(&out.stdout);
    assert!(version_str.contains("fake-proc-macro-srv 1.0.0"));

    // Test secret scrubbing and environment setup
    let out = std::process::Command::new(&wrapper)
        .env("PROD_CODE_AUTH_TOKEN", "prod-auth-token-xyz")
        .env("PROD_CODE_AUTH_TOKEN_FILE", "/tmp/token.secret")
        .env("PROD_CODE_TOKEN", "super-secret-token")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("AUTH_TOKEN=''"),
        "Must scrub PROD_CODE_AUTH_TOKEN: {stdout}"
    );
    assert!(
        stdout.contains("AUTH_FILE=''"),
        "Must scrub PROD_CODE_AUTH_TOKEN_FILE: {stdout}"
    );
    assert!(
        stdout.contains("SECRET_TOKEN=''"),
        "Must scrub PROD_CODE_TOKEN: {stdout}"
    );
    assert!(
        stdout.contains("INTERNAL='this is unstable'"),
        "Must export internal authorization: {stdout}"
    );
    assert!(
        stdout.contains("/scratch"),
        "Must set isolated scratch TMPDIR: {stdout}"
    );

    // Test memory limit validation bounds
    assert!(
        prepare_sandboxed_srv(&fake_srv, 32).is_err(),
        "Must reject < 64MB limit"
    );
}

#[cfg(unix)]
#[test]
fn test_proc_macro_concurrent_launcher_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let fake_srv = temp.path().join("fake-concurrent-srv.sh");
    std::fs::write(
        &fake_srv,
        r#"#!/bin/sh
exit 0
"#,
    )
    .unwrap();

    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&fake_srv).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_srv, perms).unwrap();

    let srv_path = Arc::new(fake_srv);
    let mut handles = vec![];

    for _ in 0..10 {
        let srv = Arc::clone(&srv_path);
        handles.push(std::thread::spawn(move || {
            prepare_sandboxed_srv(&srv, 1024).expect("Concurrent wrapper preparation must succeed")
        }));
    }

    let mut results = vec![];
    for h in handles {
        results.push(h.join().unwrap());
    }

    assert_eq!(results.len(), 10);
    let first = &results[0];
    for path in &results {
        assert_eq!(
            path, first,
            "All concurrent preparations must converge on identical validated launcher"
        );
        assert!(path.exists());
    }
}

#[cfg(unix)]
#[test]
fn test_cached_wrapper_reinstalls_on_outdated_content() {
    let temp = tempfile::tempdir().unwrap();
    let fake_srv = temp.path().join("fake-outdated-srv.sh");
    std::fs::write(&fake_srv, "#!/bin/sh\nexit 0\n").unwrap();

    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&fake_srv).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_srv, perms).unwrap();

    let wrapper_path = prepare_sandboxed_srv(&fake_srv, 1024).unwrap();
    assert!(wrapper_path.exists());
    let original_content = std::fs::read_to_string(&wrapper_path).unwrap();
    assert!(original_content.contains("exit 125"));

    // Simulate an outdated pre-fix wrapper on disk with mode 0700
    let outdated_script = "#!/bin/sh\nulimit -v 1048576 2>/dev/null || true\n";
    std::fs::write(&wrapper_path, outdated_script).unwrap();
    let mut perms = std::fs::metadata(&wrapper_path).unwrap().permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(&wrapper_path, perms).unwrap();

    // Preparing the sandbox again must detect the outdated content and reinstall
    let repaired_path = prepare_sandboxed_srv(&fake_srv, 1024).unwrap();
    assert_eq!(repaired_path, wrapper_path);
    let repaired_content = std::fs::read_to_string(&repaired_path).unwrap();
    assert_eq!(
        repaired_content, original_content,
        "Reinstalled wrapper must match fresh script exactly"
    );
    assert!(
        repaired_content.contains("exit 125"),
        "Must reinstall fail-closed error handling"
    );
    assert!(
        !repaired_content.contains("ulimit -v 1048576 2>/dev/null || true"),
        "Must replace outdated script content"
    );
}
