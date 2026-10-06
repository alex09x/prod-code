/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::time::Duration;

use super::super::in_place::run_in_place;
use super::super::overlay::clean;
use super::fixtures::{
    delta, in_place_lock_users, job, output, refused_with, snapshot, wait_until,
};

#[tokio::test]
async fn cancel_and_timeout_still_restore_in_place() {
    let ws = tempfile::tempdir().unwrap();
    let w = ws.path();
    std::fs::write(w.join("a.txt"), "base\n").unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let ctl = tempfile::tempdir().unwrap();
    let before = snapshot(w);
    let files = vec![
        delta("a.txt", Some("proposed\n")),
        delta("new/b.txt", Some("new\n")),
    ];
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let mut slow = job(w, shadow.path(), "slow", files.clone(), &["sleep", "30"]);
    slow.timeout = Duration::from_millis(300);
    let r = run_in_place(slow, rx).await;
    assert!(r.timed_out && r.error.is_none(), "{:?}", r.error);
    assert_eq!(snapshot(w), before, "not restored after the timeout");

    let c = ctl.path().to_str().unwrap().to_string();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let left = tokio::spawn(run_in_place(
        job(
            w,
            shadow.path(),
            "left",
            files,
            &["sh", "-c", ": > \"$1/started\"; exec sleep 30", "sh", &c],
        ),
        rx,
    ));
    wait_until("the command", || ctl.path().join("started").exists()).await;
    tx.send(true).unwrap();
    let r = left.await.unwrap();
    assert_eq!(r.error.as_deref(), Some("cancelled: the client left"));
    assert_eq!(snapshot(w), before, "not restored after the client left");
    assert_eq!(in_place_lock_users(w), None);
}

/// Records what `a.txt` holds, says it started, and holds the workspace until the test
/// releases it: `$1` is the control directory, `$2` the hypothesis's tag.
const HOLD: &str = "cat a.txt > \"$1/$2-before\"; : > \"$1/$2-started\"; i=0; \
                    while [ ! -e \"$1/release\" ]; do i=$((i+1)); [ $i -lt 3000 ] || exit 9; \
                    sleep 0.01; done; cat a.txt > \"$1/$2-after\"";

/// In-place hypotheses of two requests on one workspace, the second naming it through a
/// symlink, take turns: the second writes nothing until the first has run and restored.
/// Before, `run_shadow` serialized only the hypotheses of one request, so the second
/// replaced the first's file under its running command.
#[tokio::test]
async fn in_place_runs_on_one_workspace_take_turns_across_requests() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    return;

    let root = tempfile::tempdir().unwrap();
    let ws = root.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("a.txt"), "base\n").unwrap();
    let alias = root.path().join("alias");
    symlink(&ws, &alias).unwrap();
    let ctl = tempfile::tempdir().unwrap();
    let c = ctl.path().to_str().unwrap().to_string();
    let seen = |name: &str| std::fs::read_to_string(ctl.path().join(name)).ok();
    let shadow = tempfile::tempdir().unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let first = tokio::spawn(run_in_place(
        job(
            &ws,
            shadow.path(),
            "first",
            vec![delta("a.txt", Some("one\n"))],
            &["sh", "-c", HOLD, "sh", &c, "first"],
        ),
        rx.clone(),
    ));
    wait_until("the first command", || seen("first-started").is_some()).await;
    let second = tokio::spawn(run_in_place(
        job(
            &alias,
            shadow.path(),
            "second",
            vec![delta("a.txt", Some("two\n"))],
            &["sh", "-c", "cat a.txt > \"$1/second-saw\"", "sh", &c],
        ),
        rx,
    ));
    // Waiting for the lock the first holds or, unserialized, already running.
    wait_until("the second hypothesis", || {
        in_place_lock_users(&ws) == Some(2) || seen("second-saw").is_some()
    })
    .await;
    assert_eq!(
        seen("second-saw"),
        None,
        "the second hypothesis ran while the first held the workspace"
    );
    assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "one\n");
    std::fs::write(ctl.path().join("release"), "").unwrap();
    let first = first.await.unwrap();
    let second = second.await.unwrap();
    assert!(clean(&first), "{:?} {}", first.error, output(&first));
    assert!(clean(&second), "{:?} {}", second.error, output(&second));
    assert_eq!(seen("first-before").as_deref(), Some("one\n"));
    assert_eq!(seen("first-after").as_deref(), Some("one\n"));
    assert_eq!(seen("second-saw").as_deref(), Some("two\n"));
    assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "base\n");
    assert_eq!(
        in_place_lock_users(&ws),
        None,
        "the lock entry outlived its users"
    );
}

/// A workspace held by an in-place run does not hold up another one, and a hypothesis
/// waiting for its turn leaves with its client, having written nothing.
#[tokio::test]
async fn other_workspaces_proceed_and_a_waiting_hypothesis_can_be_cancelled() {
    let held = tempfile::tempdir().unwrap();
    let free = tempfile::tempdir().unwrap();
    for ws in [held.path(), free.path()] {
        std::fs::write(ws.join("a.txt"), "base\n").unwrap();
    }
    let ctl = tempfile::tempdir().unwrap();
    let c = ctl.path().to_str().unwrap().to_string();
    let shadow = tempfile::tempdir().unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let first = tokio::spawn(run_in_place(
        job(
            held.path(),
            shadow.path(),
            "first",
            vec![delta("a.txt", Some("one\n"))],
            &["sh", "-c", HOLD, "sh", &c, "first"],
        ),
        rx.clone(),
    ));
    wait_until("the first command", || {
        ctl.path().join("first-started").exists()
    })
    .await;

    let other = tokio::time::timeout(
        Duration::from_secs(30),
        run_in_place(
            job(
                free.path(),
                shadow.path(),
                "other",
                vec![delta("a.txt", Some("other\n"))],
                &["cat", "a.txt"],
            ),
            rx.clone(),
        ),
    )
    .await
    .expect("another workspace waited for this one's lock");
    assert!(clean(&other), "{:?}", other.error);
    assert_eq!(output(&other), "other\n");
    assert_eq!(in_place_lock_users(free.path()), None);

    let (tx, cancel) = tokio::sync::watch::channel(false);
    let waiting = tokio::spawn(run_in_place(
        job(
            held.path(),
            shadow.path(),
            "waiting",
            vec![delta("a.txt", Some("never\n"))],
            &["sh", "-c", "echo ran"],
        ),
        cancel,
    ));
    wait_until("the waiting hypothesis", || {
        in_place_lock_users(held.path()) == Some(2)
    })
    .await;
    tx.send(true).unwrap();
    let waiting = waiting.await.unwrap();
    refused_with("waiting", &waiting, "cancelled: the client left");
    assert_eq!(in_place_lock_users(held.path()), Some(1));
    assert_eq!(
        std::fs::read_to_string(held.path().join("a.txt")).unwrap(),
        "one\n"
    );

    std::fs::write(ctl.path().join("release"), "").unwrap();
    let first = first.await.unwrap();
    assert!(clean(&first), "{:?} {}", first.error, output(&first));
    assert_eq!(
        std::fs::read_to_string(held.path().join("a.txt")).unwrap(),
        "base\n"
    );
    assert_eq!(in_place_lock_users(held.path()), None);
}
