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

mod sweeps;

#[tokio::test]
async fn pressure_redirects_active_sessions_to_a_compatible_peer_including_swift() {
    crate::test_cache_support::isolated!();
    for (engine, platform) in [("unavailable-test-engine", "linux"), ("swift", "macos")] {
        let temp = tempfile::tempdir().unwrap();
        let mut state = ServerState::new(temp.path().join("storage"));
        state.engine_allowlist = vec!["unavailable-test-engine".into()];
        state
            .metrics
            .record_inventory(prod_code_protocol::ToolchainInventory {
                ts_ms: 0,
                node: "sample".into(),
                engines: vec![],
            });
        *state.advertise.write().await = "127.0.0.1:9400".into();
        let workspace = Arc::new(crate::workspace::shared::SharedWorkspace::new(
            temp.path().join("storage/active"),
            engine.into(),
            None,
            None,
            None,
            None,
        ));
        workspace.active_sessions.store(1, Ordering::Relaxed);
        let mut redirects = workspace.rebalance_tx.subscribe();
        state.workspace_manager.workspaces.write().await.insert(
            workspace.key.clone(),
            crate::workspace::loader::LoadState::Ready(Arc::clone(&workspace)),
        );
        let peer = crate::tests::common::peer("127.0.0.2:9400", platform, &[engine], 0.0);
        state.cluster.write().await.insert(
            peer.addr.clone(),
            PeerEntry {
                gossip: NodeGossip {
                    addr: peer.addr,
                    status: peer.status,
                    workspaces: vec![],
                    peers: vec![],
                    sent_at_ms: 0,
                },
                last_seen: Instant::now(),
            },
        );
        let state = Arc::new(state);
        let task = tokio::spawn(janitor_with_probe(
            Arc::clone(&state),
            0,
            0,
            0,
            0,
            Duration::from_millis(5),
            |_| prod_code_protocol::HostResources {
                memory_total_bytes: Some(100),
                memory_available_bytes: Some(0),
                storage_free_millis: Some(1000),
            },
        ));
        let redirect = tokio::time::timeout(Duration::from_secs(5), redirects.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            redirect,
            (
                "127.0.0.2:9400".into(),
                Some("evacuating node under memory pressure".into())
            )
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(workspace.active_sessions.load(Ordering::Relaxed), 1);
        assert!(!workspace.unloaded.load(Ordering::Relaxed));
    }
}

#[tokio::test]
async fn janitor_evicts_idle_workspaces_prunes_old_copies_and_records_gateway_counts() {
    crate::test_cache_support::isolated!();
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path().join("storage");
    std::fs::create_dir(&storage).unwrap();
    let old_copy = storage.join("sample--wt-old");
    let old_main = storage.join("unused-main");
    for root in [&old_copy, &old_main] {
        std::fs::create_dir(root).unwrap();
        let marker = root.join(crate::workspace::prune::LAST_USED_MARKER);
        std::fs::write(&marker, "").unwrap();
        std::fs::File::open(marker)
            .unwrap()
            .set_modified(std::time::SystemTime::UNIX_EPOCH)
            .unwrap();
    }
    let mut state = ServerState::new(storage.clone());
    state.engine_allowlist = vec!["unavailable-test-engine".into()];
    state.build_cache_ram = true;
    state.build_cache_dir = Some(temp.path().join("build-cache"));
    state
        .metrics
        .record_inventory(prod_code_protocol::ToolchainInventory {
            ts_ms: 0,
            node: "sample".into(),
            engines: vec![],
        });
    state.metrics.set_node("sample");
    let manager = Arc::clone(&state.workspace_manager);
    let workspace = Arc::new(crate::workspace::shared::SharedWorkspace::new(
        storage.join("loaded"),
        "text".into(),
        None,
        None,
        None,
        None,
    ));
    workspace.last_used.store(0, Ordering::Relaxed);
    manager.workspaces.write().await.insert(
        workspace.key.clone(),
        crate::workspace::loader::LoadState::Ready(Arc::clone(&workspace)),
    );
    let state = Arc::new(state);
    let calls = Arc::new(AtomicUsize::new(0));
    let probe_calls = Arc::clone(&calls);
    let task = tokio::spawn(janitor_with_probe(
        Arc::clone(&state),
        1,
        1,
        1,
        1,
        Duration::from_millis(5),
        move |_| {
            let index = probe_calls.fetch_add(1, Ordering::Relaxed);
            prod_code_protocol::HostResources {
                memory_total_bytes: Some(100),
                memory_available_bytes: Some(if index == 1 { 0 } else { 100 }),
                storage_free_millis: Some(1000),
            }
        },
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while calls.load(Ordering::Relaxed) < 3
            || old_copy.exists()
            || old_main.exists()
            || manager.loaded_count().await != 0
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(workspace.unloaded.load(Ordering::Relaxed));
    assert!(!old_copy.exists());
    assert!(!old_main.exists());
    let snapshot = state.metrics.latest_snapshot().unwrap();
    assert_eq!(snapshot.node, "sample");
    assert_eq!(snapshot.active_sessions, 0);
    assert_eq!(snapshot.workspace_count, 0);
    assert_eq!(snapshot.engine_count, 0);
}

#[test]
fn idle_engines_are_unloaded_after_the_configured_time_cut_to_the_pressure_limit() {
    assert_eq!(PRESSURE_EVICT_IDLE, Duration::from_secs(300));
    // (--idle-evict-secs, host short of memory, after how many seconds)
    for (configured, short, expected) in [
        (0, false, None),
        (0, true, Some(300)),
        (60, false, Some(60)),
        (60, true, Some(60)),
        (300, true, Some(300)),
        (900, false, Some(900)),
        (900, true, Some(300)),
    ] {
        assert_eq!(
            evict_after(configured, short),
            expected.map(Duration::from_secs),
            "{configured} {short}"
        );
    }
}

#[tokio::test]
async fn the_first_sweep_collects_the_inventory_prunes_old_metric_logs_and_keeps_active_work() {
    crate::test_cache_support::isolated!();
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path().join("storage");
    std::fs::create_dir(&storage).unwrap();
    let mut state = ServerState::new(storage.clone());
    state.engine_allowlist = vec!["unavailable-test-engine".into()];
    state.metrics.set_node("sample");
    let metrics_dir = state.metrics.dir().to_path_buf();
    std::fs::create_dir_all(&metrics_dir).unwrap();
    let expired = metrics_dir.join("events-2000-01-01.jsonl");
    let unrelated = metrics_dir.join("events-notes.txt");
    std::fs::write(&expired, "{}\n").unwrap();
    std::fs::write(&unrelated, "keep").unwrap();

    // Two loaded workspaces on a node short of memory, and no peer to hand either to: the one in
    // use stays where it is, and the idle one is not unloaded before its time.
    let mut loaded = Vec::new();
    for (name, active) in [("busy", 2), ("quiet", 0)] {
        let workspace = Arc::new(crate::workspace::shared::SharedWorkspace::new(
            storage.join(name),
            "text".into(),
            None,
            None,
            None,
            None,
        ));
        workspace.active_sessions.store(active, Ordering::Relaxed);
        state.workspace_manager.workspaces.write().await.insert(
            workspace.key.clone(),
            crate::workspace::loader::LoadState::Ready(Arc::clone(&workspace)),
        );
        loaded.push(workspace);
    }
    let mut redirects = loaded[0].rebalance_tx.subscribe();
    let state = Arc::new(state);
    assert!(state.metrics.toolchain_inventory().is_none());
    let task = tokio::spawn(janitor_with_probe(
        Arc::clone(&state),
        0,
        0,
        0,
        0,
        Duration::from_millis(5),
        |_| prod_code_protocol::HostResources {
            memory_total_bytes: Some(100),
            memory_available_bytes: Some(0),
            storage_free_millis: Some(1000),
        },
    ));
    tokio::time::timeout(Duration::from_secs(10), async {
        while state.metrics.toolchain_inventory().is_none() || expired.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    // Let a few more sweeps run under pressure before looking.
    tokio::time::sleep(Duration::from_millis(100)).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());

    let inventory = state.metrics.toolchain_inventory().unwrap();
    assert_eq!(inventory.node, "sample");
    // Nothing is advertised, so no engine is reported as available whatever the host has.
    assert!(inventory.engines.iter().all(|engine| !engine.available));
    assert!(!expired.exists());
    assert_eq!(std::fs::read_to_string(&unrelated).unwrap(), "keep");
    assert!(redirects.try_recv().is_err());
    assert_eq!(state.workspace_manager.loaded_count().await, 2);
    for workspace in &loaded {
        assert!(!workspace.unloaded.load(Ordering::Relaxed));
    }
    assert_eq!(loaded[0].active_sessions.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn the_gateway_janitor_waits_a_full_period_before_its_first_sweep() {
    crate::test_cache_support::isolated!();
    let temp = tempfile::tempdir().unwrap();
    let state = Arc::new(ServerState::new(temp.path().join("storage")));
    let task = tokio::spawn(janitor(Arc::clone(&state), 0, 0, 0, 0));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!task.is_finished());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(state.metrics.latest_snapshot().is_none());
    assert!(state.metrics.toolchain_inventory().is_none());
}
