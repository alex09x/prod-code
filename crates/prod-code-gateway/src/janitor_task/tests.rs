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
