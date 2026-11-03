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
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[cfg(unix)]
#[test]
fn descendant_retaining_stdout_pipe_is_terminated_and_reaped_on_timeout() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("descendant.pid");

    // Parent exits 0 immediately, background child retains stdout via inherited descriptor.
    let fake_npm = temp.path().join("fake_npm_pipe_leak");
    let script = format!(
        "sleep 60 & echo $! > \"{}\"\nexit 0",
        pid_file.to_string_lossy()
    );
    make_executable_script(&fake_npm, &script);

    let start = Instant::now();
    let result = probe_npm_root(&fake_npm, Duration::from_millis(200), []);
    let elapsed = start.elapsed();

    // 1. Must timeout promptly, not hang for 60s
    assert_eq!(result, None, "probe must return None on timeout");
    assert!(
        elapsed < Duration::from_secs(3),
        "finite deadline must prevent hanging on descendant pipe; took {elapsed:?}"
    );

    // 2. Read descendant PID
    let pid_str = std::fs::read_to_string(&pid_file).expect("descendant pid file must exist");
    let descendant_pid: i32 = pid_str.trim().parse().expect("valid descendant pid");

    // 3. Observable verification that descendant was terminated
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let is_alive = |pid: i32| unsafe { kill(pid, 0) == 0 };

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut retired = !is_alive(descendant_pid);
    while !retired && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        retired = !is_alive(descendant_pid);
    }

    assert!(
        retired,
        "descendant {descendant_pid} holding stdout pipe must be terminated, not left alive"
    );
}

#[cfg(unix)]
#[test]
fn native_tsgo_fast_path_is_lazy_with_observable_invocation_check() {
    let temp = tempfile::tempdir().unwrap();
    let marker_file = temp.path().join("npm_invoked_marker.txt");
    let fake_npm = temp.path().join("fake_npm");
    make_executable_script(
        &fake_npm,
        &format!(
            "echo invoked > \"{}\"\nexit 0",
            marker_file.to_string_lossy()
        ),
    );

    let fake_tsgo = temp.path().join("tsgo");
    std::fs::write(&fake_tsgo, "").unwrap();

    let npm_invoked_counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = npm_invoked_counter.clone();

    // Fast path: tsgo is found. Resolver must NOT be called.
    let resolved = native_typescript_lsp_with_resolver(Some(fake_tsgo.clone()), || {
        counter_clone.fetch_add(1, Ordering::SeqCst);
        let _ = probe_npm_root(&fake_npm, Duration::from_secs(1), []);
        None
    });

    assert_eq!(resolved, Some(fake_tsgo));
    assert_eq!(
        npm_invoked_counter.load(Ordering::SeqCst),
        0,
        "npm resolver must not be invoked when tsgo is found"
    );
    assert!(
        !marker_file.exists(),
        "npm executable must not be run when tsgo is found (marker must not exist)"
    );

    // Fallback path: tsgo is None. Resolver must be called.
    let not_found = native_typescript_lsp_with_resolver(None, || {
        counter_clone.fetch_add(1, Ordering::SeqCst);
        let _ = probe_npm_root(&fake_npm, Duration::from_secs(1), []);
        None
    });
    assert_eq!(not_found, None);
    assert_eq!(
        npm_invoked_counter.load(Ordering::SeqCst),
        1,
        "npm resolver must be invoked when tsgo is None"
    );
    assert!(
        marker_file.exists(),
        "marker file must exist, proving npm was invoked on the fallback path"
    );
}

#[cfg(unix)]
#[test]
fn probe_bounds_large_output_without_pipe_backpressure_hanging() {
    let temp = tempfile::tempdir().unwrap();
    let fake_root = temp.path().join("big_output_modules");
    std::fs::create_dir_all(&fake_root).unwrap();

    let fake_npm = temp.path().join("fake_npm_pipe_stress");
    // Generate 128KB of output followed by valid directory path.
    // 128KB exceeds standard OS pipe buffer (typically 64KB), which would deadlock
    // if stdout were not drained concurrently while waiting for child exit.
    make_executable_script(
        &fake_npm,
        &format!(
            "echo \"{}\"\nhead -c 131072 /dev/zero | tr '\\0' 'x'\nexit 0",
            fake_root.to_string_lossy()
        ),
    );

    let start = Instant::now();
    let result = probe_npm_root(&fake_npm, Duration::from_secs(2), []);
    let elapsed = start.elapsed();

    assert_eq!(result, Some(fake_root));
    assert!(
        elapsed < Duration::from_secs(2),
        "pipe backpressure must not hang caller; took {elapsed:?}"
    );
}

#[cfg(unix)]
#[test]
fn wait_with_timeout_terminates_descendant_even_after_parent_exits() {
    let temp = tempfile::tempdir().unwrap();
    let pid_file = temp.path().join("wait_descendant.pid");

    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.args([
        "-c",
        &format!(
            "sleep 60 & echo $! > \"{}\"; exit 0",
            pid_file.to_string_lossy()
        ),
    ])
    .stderr(std::process::Stdio::null());

    let group = OwnedProcessGroup::spawn(&mut cmd).expect("spawn controlled test child");

    let start = Instant::now();
    let output = wait_with_timeout(group, Duration::from_millis(150));
    let elapsed = start.elapsed();

    assert!(output.is_none(), "timed-out wait must return None");
    assert!(
        elapsed < Duration::from_secs(2),
        "wait_with_timeout must abort promptly on finite deadline; took {elapsed:?}"
    );

    let pid_str = std::fs::read_to_string(&pid_file).expect("descendant pid file must exist");
    let descendant_pid: i32 = pid_str.trim().parse().expect("valid descendant pid");

    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let is_alive = |pid: i32| unsafe { kill(pid, 0) == 0 };

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut retired = !is_alive(descendant_pid);
    while !retired && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        retired = !is_alive(descendant_pid);
    }

    assert!(
        retired,
        "descendant {descendant_pid} must be terminated by wait_with_timeout"
    );
}
