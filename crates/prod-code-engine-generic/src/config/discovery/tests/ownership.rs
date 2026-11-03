/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The probe owns its process group until the leader is reaped (#1794).

use super::*;
use crate::config::discovery::process::audit;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn sh(script: &str) -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", script])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

/// The leader exits at once while a descendant keeps stdout open. The leader must not
/// be reaped before EOF, so at the deadline the group signal goes to a PID the leader
/// (now a zombie) still holds, never to a released and possibly reused one.
#[cfg(unix)]
#[test]
fn timeout_signals_the_group_only_while_the_leader_is_unreaped() {
    let _ = audit::take();
    let group = OwnedProcessGroup::spawn(&mut sh("sleep 30 & exit 0")).unwrap();
    let leader = i32::try_from(group.id()).unwrap();
    // Let the leader exit well before the deadline.
    let output = wait_with_timeout(group, Duration::from_millis(400));
    assert!(
        output.is_none(),
        "stdout never reached EOF, so the wait times out"
    );

    let signals = audit::take();
    assert_eq!(signals.len(), 1, "exactly one group signal: {signals:?}");
    assert_eq!(signals[0].pgid, leader);
    assert!(
        signals[0].leader_held,
        "the leader was reaped before its group was signalled: {signals:?}"
    );
}

/// A completed run reaps its leader and sends no signal at all.
#[cfg(unix)]
#[test]
fn completed_run_sends_no_signal() {
    let _ = audit::take();
    let group = OwnedProcessGroup::spawn(&mut sh("echo done")).unwrap();
    let output = wait_with_timeout(group, Duration::from_secs(5)).expect("run completes");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"done\n");
    assert!(audit::take().is_empty());
}

/// A deadline that has already passed still gets one poll: a child that finished and
/// closed stdout is reported, not killed as expired.
#[cfg(unix)]
#[test]
fn expired_deadline_is_polled_once_before_giving_up() {
    let _ = audit::take();
    let group = OwnedProcessGroup::spawn(&mut sh("echo finished")).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let output = group
        .wait_with_deadline(Instant::now() - Duration::from_millis(1))
        .expect("a finished child is reported even past the deadline");
    assert_eq!(output.stdout, b"finished\n");
    assert!(
        audit::take().is_empty(),
        "a finished child is not signalled"
    );
}

/// Output beyond the cap is drained, not stored, and the run still completes.
#[cfg(unix)]
#[test]
fn output_collection_is_bounded() {
    let group = OwnedProcessGroup::spawn(&mut sh("head -c 200000 /dev/zero | tr '\\0' x")).unwrap();
    let output = wait_with_timeout(group, Duration::from_secs(5)).expect("run completes");
    assert_eq!(output.stdout.len(), MAX_PROBE_STDOUT_BYTES);
}
