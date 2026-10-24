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
use std::os::unix::process::{CommandExt, ExitStatusExt};

#[test]
fn run_dir_is_the_subdir_only_when_it_is_a_real_directory_inside_the_workspace() {
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("crates/app")).unwrap();
    std::fs::write(ws.path().join("file.txt"), "x").unwrap();
    for (subdir, expected) in [
        (None, ""),
        (Some(""), ""),
        (Some("crates/app"), "crates/app"),
        (Some("crates"), "crates"),
        (Some("missing"), ""),
        (Some("file.txt"), ""),
        (Some("/tmp"), ""),
        (Some("crates/../crates"), ""),
        (Some(".."), ""),
    ] {
        assert_eq!(
            resolve_run_dir(ws.path(), subdir),
            ws.path().join(expected),
            "{subdir:?}"
        );
    }
}

#[test]
fn programs_are_found_on_the_path_only_when_they_are_files() {
    assert!(on_path("sh"));
    assert!(!on_path("no-such-program-on-any-node"));
}

#[test]
fn a_group_is_killed_unless_its_leader_was_already_reaped() {
    let mut child = std::process::Command::new("sh")
        .args(["-c", "sleep 30 & wait"])
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id();
    // Reaped already: nothing is signalled.
    kill_exec_group(pid, &std::sync::Mutex::new(true));
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the child must still run"
    );
    kill_exec_group(pid, &std::sync::Mutex::new(false));
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(9));
}

#[test]
fn waiting_reports_the_raw_status_and_marks_the_child_exited() {
    let child = std::process::Command::new("sh")
        .args(["-c", "exit 3"])
        .spawn()
        .unwrap();
    let exited = std::sync::Mutex::new(false);
    let (raw, _usage) = wait_with_usage(child.id() as i32, &exited).expect("a status");
    assert_eq!(std::process::ExitStatus::from_raw(raw).code(), Some(3));
    assert!(*exited.lock().unwrap());
    // The pid is reaped now, so there is nothing left to wait for.
    assert!(wait_with_usage(child.id() as i32, &exited).is_none());
}

#[tokio::test]
async fn both_pipes_are_read_in_order_with_their_stream_flag() {
    let mut child = std::process::Command::new("sh")
        .args(["-c", "printf out; printf err >&2"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, mut rx) = rapidfire::mpsc::bounded(16);
    let readers = spawn_pipe_readers(&mut child, tx);
    assert_eq!(readers.len(), 2);
    for reader in readers {
        reader.await.unwrap();
    }
    let mut got = Vec::new();
    while let Ok(chunk) = rx.recv().await {
        got.push((chunk.stderr, chunk.data.unwrap()));
    }
    got.sort();
    assert_eq!(got, vec![(false, b"out".to_vec()), (true, b"err".to_vec())]);
    child.wait().unwrap();
    // A child without piped output has no readers.
    let mut quiet = std::process::Command::new("true").spawn().unwrap();
    let (tx, _rx) = rapidfire::mpsc::bounded(1);
    assert!(spawn_pipe_readers(&mut quiet, tx).is_empty());
    quiet.wait().unwrap();
}

#[test]
fn disk_headroom_on_a_missing_path_falls_back_to_the_storage_root() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("no/such/workspace");
    // Whatever the free space of the machine, the outcome names the path it measured.
    match check_node_disk_headroom(&missing, root.path()) {
        DiskCheckOutcome::Ok => {}
        DiskCheckOutcome::Warn(w) => assert!(w.starts_with("[prod-code exec] WARNING: "), "{w}"),
        DiskCheckOutcome::Refuse(r) => {
            assert!(r.starts_with("refused: node "), "{r}");
            assert!(r.contains(&missing.display().to_string()), "{r}");
        }
    }
}
