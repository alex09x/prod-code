/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #2068: an in-place shadow run and a sync of the same workspace copy never overlap, and a
//! request for a RAM overlay is refused on a node that would run it in place.

use std::time::Duration;

use super::super::in_place::run_in_place;
use super::super::runner::ram_refusal;
use super::fixtures::{delta, job, snapshot, wait_until};

/// While a sync holds the workspace, the hypothesis writes nothing and runs nothing; once the
/// sync is done it runs and puts the copy back. Before, a concurrent sync rewrote the
/// hypothesis's files under its command and the restore refused them.
#[tokio::test]
async fn an_in_place_hypothesis_waits_for_a_sync_of_its_workspace() {
    let ws = tempfile::tempdir().unwrap();
    let w = ws.path();
    std::fs::write(w.join("a.txt"), "base\n").unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let ctl = tempfile::tempdir().unwrap();
    let before = snapshot(w);

    let sync = crate::sync::server_workspace_sync_lock(w)
        .lock_owned()
        .await;
    let c = ctl.path().to_str().unwrap().to_string();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let run = tokio::spawn(run_in_place(
        job(
            w,
            shadow.path(),
            "waits",
            vec![delta("a.txt", Some("proposed\n"))],
            &["sh", "-c", "cat a.txt > \"$1/seen\"", "sh", &c],
        ),
        rx,
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!ctl.path().join("seen").exists(), "ran under a sync");
    assert_eq!(snapshot(w), before, "wrote into the copy under a sync");

    drop(sync);
    let r = run.await.unwrap();
    assert!(r.error.is_none(), "{:?}", r.error);
    wait_until("the command", || ctl.path().join("seen").exists()).await;
    assert_eq!(
        std::fs::read_to_string(ctl.path().join("seen")).unwrap(),
        "proposed\n"
    );
    assert_eq!(snapshot(w), before, "not restored");
}

/// A hypothesis waiting for a sync stops when the client leaves, and writes nothing.
#[tokio::test]
async fn a_hypothesis_waiting_for_a_sync_stops_when_the_client_leaves() {
    let ws = tempfile::tempdir().unwrap();
    let w = ws.path();
    std::fs::write(w.join("a.txt"), "base\n").unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let before = snapshot(w);

    let _sync = crate::sync::server_workspace_sync_lock(w)
        .lock_owned()
        .await;
    let (tx, rx) = tokio::sync::watch::channel(false);
    let run = tokio::spawn(run_in_place(
        job(
            w,
            shadow.path(),
            "left",
            vec![delta("a.txt", Some("proposed\n"))],
            &["true"],
        ),
        rx,
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    tx.send(true).unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("still waiting after the client left")
        .unwrap();
    assert_eq!(r.error.as_deref(), Some("cancelled: the client left"));
    assert_eq!(snapshot(w), before);
}

#[test]
fn a_ram_request_is_refused_where_shadows_would_run_in_place() {
    let refusal = ram_refusal(true, Some("user namespaces are restricted")).unwrap();
    assert!(
        refusal.contains("user namespaces are restricted"),
        "{refusal}"
    );
    assert!(refusal.contains("shared workspace copy"), "{refusal}");
    assert_eq!(ram_refusal(true, None), None);
    assert_eq!(ram_refusal(false, Some("no overlay")), None);
}
