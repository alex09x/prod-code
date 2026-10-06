/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::fixtures::{SlowLoader, host, loaded, roomy_admission, slow_loader};
use crate::workspace::loader::LoadState;
use crate::workspace::manager::WorkspaceManager;
use crate::workspace::session::WorkspaceLease;
use crate::workspace::types::WorkspaceKey;

/// A leader whose client gives up while its Rust engine loads cancels only its wait
/// (#433): the load goes on holding its reservation, a follower arriving meanwhile is
/// answered by that same load instead of waiting on a Loading entry forever, and no session
/// is left counted for the leader that went away.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_canceled_leader_leaves_its_load_running_reserved_and_answering() {
    let SlowLoader {
        load,
        mut started,
        gate,
        loads,
    } = slow_loader();
    let admission = roomy_admission();
    let manager =
        Arc::new(WorkspaceManager::with_admission(Arc::clone(&admission)).with_rust_loader(load));
    let root = PathBuf::from("/srv/ws/slow");
    let attach = || {
        let manager = Arc::clone(&manager);
        let root = root.clone();
        tokio::spawn(async move { manager.get_or_load(&root, "rust").await })
    };

    let leader = attach();
    started.recv().await.expect("the load started");
    leader.abort();
    assert!(leader.await.err().is_some_and(|err| err.is_cancelled()));
    assert_eq!(
        admission.reserved_bytes(),
        2 << 30,
        "the running load keeps its reservation"
    );
    assert!(
        manager.is_loaded(&root).await,
        "the load is still in flight"
    );

    let follower = attach();
    gate.send(()).unwrap();
    let ws = tokio::time::timeout(Duration::from_secs(30), follower)
        .await
        .expect("the follower was answered")
        .unwrap()
        .expect("the load completed");
    assert_eq!(loads.load(Ordering::SeqCst), 1, "one load for both");
    assert_eq!(
        ws.active_sessions.load(Ordering::Relaxed),
        1,
        "only the follower's session is counted"
    );
    assert_eq!(
        admission.reserved_bytes(),
        0,
        "returned once the load ended"
    );
    assert!(manager.get_loaded(&root).await.is_some());
}

/// Concurrent cold engine loads across distinct workspaces are bounded by the semaphore (#408):
/// when the bound is 2, only 2 out of 4 concurrent loads start compilation, and the remaining 2
/// wait on the semaphore until permits are released.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_engine_loads_are_bounded_by_semaphore() {
    let SlowLoader {
        load,
        mut started,
        gate,
        loads,
    } = slow_loader();
    let manager = Arc::new(
        WorkspaceManager::with_admission(roomy_admission())
            .with_max_concurrent_loads(2)
            .with_rust_loader(load),
    );

    let mut tasks = Vec::new();
    for i in 1..=4 {
        let manager = Arc::clone(&manager);
        let root = PathBuf::from(format!("/srv/ws/bounded-{i}"));
        tasks.push(tokio::spawn(async move {
            manager.get_or_load(&root, "rust").await
        }));
    }

    // Exactly 2 loads acquire the semaphore and start compilation.
    started.recv().await.expect("first load started");
    started.recv().await.expect("second load started");

    // The third and fourth loads are queued and blocked on the semaphore.
    let timeout = tokio::time::timeout(Duration::from_millis(50), started.recv()).await;
    assert!(timeout.is_err(), "third load must wait on semaphore");
    assert_eq!(loads.load(Ordering::SeqCst), 2);

    // Release one permit: third load starts.
    gate.send(()).unwrap();
    started.recv().await.expect("third load started");
    assert_eq!(loads.load(Ordering::SeqCst), 3);

    // Release another permit: fourth load starts.
    gate.send(()).unwrap();
    started.recv().await.expect("fourth load started");
    assert_eq!(loads.load(Ordering::SeqCst), 4);

    // Release the remaining two permits.
    gate.send(()).unwrap();
    gate.send(()).unwrap();

    for task in tasks {
        let res = task.await.unwrap();
        assert!(res.is_ok());
    }
}

/// An unloaded in-flight engine cannot replace a later load of the same path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unloaded_load_cannot_publish_over_its_replacement() {
    let SlowLoader {
        load,
        mut started,
        gate,
        loads,
    } = slow_loader();
    let manager =
        Arc::new(WorkspaceManager::with_admission(roomy_admission()).with_rust_loader(load));
    let root = PathBuf::from("/srv/ws/replaced");
    let attach = || {
        let manager = Arc::clone(&manager);
        let root = root.clone();
        tokio::spawn(async move { manager.get_or_load(&root, "rust").await })
    };
    let old = attach();
    started.recv().await.unwrap();
    assert_eq!(manager.unload_under(&root).await, 1);
    let fresh = attach();
    started.recv().await.unwrap();
    gate.send(()).unwrap();
    let old_result = tokio::time::timeout(Duration::from_secs(30), old)
        .await
        .unwrap()
        .unwrap();
    assert!(
        old_result.is_err(),
        "an unloaded engine was published as ready"
    );
    assert!(
        manager.get_loaded(&root).await.is_none(),
        "the newer load is still pending"
    );
    gate.send(()).unwrap();
    let current = tokio::time::timeout(Duration::from_secs(30), fresh)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let registered = manager.get_loaded(&root).await.unwrap();
    assert!(Arc::ptr_eq(current.workspace(), &registered));
    assert_eq!(current.active_sessions.load(Ordering::Relaxed), 1);
    assert_eq!(loads.load(Ordering::SeqCst), 2);
}

/// A load that panics, here while it reads the host's memory, is answered like one that
/// failed: its leader and a follower waiting on it get the error, no Loading entry is left
/// to trap later sessions, nothing stays reserved, and the next session loads afresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_load_that_panics_answers_its_sessions_and_leaves_no_loading_entry() {
    let (started_tx, mut started) = tokio::sync::mpsc::unbounded_channel();
    let (gate, gate_rx) = std::sync::mpsc::channel::<()>();
    let gate_rx = std::sync::Mutex::new(gate_rx);
    let reads = AtomicUsize::new(0);
    let probe: crate::admission::MemoryProbe = Arc::new(move || {
        if reads.fetch_add(1, Ordering::SeqCst) == 0 {
            let _ = started_tx.send(());
            let _ = gate_rx.lock().unwrap().recv();
            panic!("scripted probe failure");
        }
        Some(host(10, 100))
    });
    let manager = Arc::new(WorkspaceManager::with_admission(Arc::new(
        crate::admission::Admission::with_probe(probe, 2048, Duration::ZERO),
    )));
    let root = PathBuf::from("/srv/ws/panics");
    let attach = || {
        let manager = Arc::clone(&manager);
        let root = root.clone();
        tokio::spawn(async move { manager.get_or_load(&root, "text").await })
    };

    let leader = attach();
    started.recv().await.expect("the load started");
    let follower = attach();
    // The leader holds one receiver of the load's broadcast; the follower adds another.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let subscribed = match manager
                .workspaces
                .read()
                .await
                .get(&WorkspaceKey(root.clone()))
            {
                Some(LoadState::Loading(tx)) => tx.receiver_count() >= 2,
                _ => false,
            };
            if subscribed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the follower waits on the load");
    gate.send(()).unwrap();

    let answered = |task: tokio::task::JoinHandle<Result<WorkspaceLease>>| async {
        tokio::time::timeout(Duration::from_secs(30), task)
            .await
            .expect("answered")
            .unwrap()
            .err()
            .map(|err| format!("{err:#}"))
    };
    let leader_err = answered(leader)
        .await
        .expect("the leader is told it failed");
    assert!(leader_err.contains("panicked"), "{leader_err}");
    let follower_err = answered(follower)
        .await
        .expect("the follower is told it failed");
    assert!(follower_err.contains("panicked"), "{follower_err}");
    assert!(
        !manager.is_loaded(&root).await,
        "no Loading entry is left behind"
    );
    assert_eq!(manager.admission().reserved_bytes(), 0);

    let ws = manager
        .get_or_load(&root, "text")
        .await
        .expect("loaded afresh");
    assert_eq!(ws.active_sessions.load(Ordering::Relaxed), 1);
}

/// The validation engine is a load like any other: a session that stops waiting leaves it
/// running with its reservation, and the next session waits for that load instead of
/// starting a second one beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_canceled_validation_session_leaves_one_load_running_with_its_reservation() {
    let SlowLoader {
        load,
        mut started,
        gate,
        loads,
    } = slow_loader();
    let admission = roomy_admission();
    let ws = loaded("/srv/ws/validated", 0);
    let validate = || {
        let ws = Arc::clone(&ws);
        let admission = Arc::clone(&admission);
        let load = Arc::clone(&load);
        tokio::spawn(async move { ws.validation_engine(&admission, load).await.is_some() })
    };

    let first = validate();
    started.recv().await.expect("the load started");
    first.abort();
    assert!(first.await.err().is_some_and(|err| err.is_cancelled()));
    assert_eq!(
        admission.reserved_bytes(),
        2 << 30,
        "the running load keeps its reservation"
    );

    let second = validate();
    gate.send(()).unwrap();
    let validated = tokio::time::timeout(Duration::from_secs(30), second)
        .await
        .expect("the second session was answered")
        .unwrap();
    assert!(
        !validated,
        "the scripted load fails: validation stays on the main engine"
    );
    assert_eq!(loads.load(Ordering::SeqCst), 1, "one load for both");
    assert_eq!(
        admission.reserved_bytes(),
        0,
        "returned once the load ended"
    );
}

/// A validation engine the host has no memory for is not loaded, and not remembered as
/// failed: a later session, once the memory is back, loads it.
#[tokio::test]
async fn a_refused_validation_engine_is_asked_for_again() {
    let SlowLoader {
        load, gate, loads, ..
    } = slow_loader();
    drop(gate);
    let admission = Arc::new(crate::admission::Admission::with_probe(
        crate::admission::scripted_probe(vec![host(90, 100), host(10, 100)]),
        2048,
        Duration::ZERO,
    ));
    let ws = loaded("/srv/ws/validated", 0);
    assert!(
        ws.validation_engine(&admission, Arc::clone(&load))
            .await
            .is_none()
    );
    assert_eq!(loads.load(Ordering::SeqCst), 0, "refused before loading");
    assert!(
        ws.validation_engine(&admission, Arc::clone(&load))
            .await
            .is_none()
    );
    assert_eq!(
        loads.load(Ordering::SeqCst),
        1,
        "asked again once memory is back"
    );
    assert_eq!(admission.reserved_bytes(), 0);
}
