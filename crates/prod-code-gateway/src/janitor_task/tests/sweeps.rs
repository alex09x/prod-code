/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The janitor's space watchdog, its evacuation fallbacks and its hourly inventory refresh.

use super::*;

const SHORT: fn(&Path) -> prod_code_protocol::HostResources =
    |_| prod_code_protocol::HostResources {
        memory_total_bytes: Some(100),
        memory_available_bytes: Some(0),
        storage_free_millis: Some(1000),
    };

fn state_with_storage(temp: &Path) -> ServerState {
    let storage = temp.join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    let mut state = ServerState::new(storage);
    state.engine_allowlist = vec!["unavailable-test-engine".into()];
    state
        .metrics
        .record_inventory(prod_code_protocol::ToolchainInventory {
            ts_ms: 0,
            node: "preset".into(),
            engines: vec![],
        });
    state
}

async fn load_active(
    state: &ServerState,
    name: &str,
) -> Arc<crate::workspace::shared::SharedWorkspace> {
    let workspace = Arc::new(crate::workspace::shared::SharedWorkspace::new(
        state.storage_root.join(name),
        "unavailable-test-engine".into(),
        None,
        None,
        None,
        None,
    ));
    workspace.active_sessions.store(1, Ordering::Relaxed);
    state.workspace_manager.workspaces.write().await.insert(
        workspace.key.clone(),
        crate::workspace::loader::LoadState::Ready(Arc::clone(&workspace)),
    );
    workspace
}

async fn add_peer(state: &ServerState, addr: &str) {
    let peer = crate::tests::common::peer(addr, "linux", &["unavailable-test-engine"], 0.0);
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
}

#[tokio::test]
async fn a_node_below_its_free_space_floor_removes_idle_worktree_copies_only() {
    crate::test_cache_support::isolated!();
    let temp = tempfile::tempdir().unwrap();
    let state = state_with_storage(temp.path());
    let storage = state.storage_root.clone();
    let (old, young, plain) = (
        storage.join("sample--wt-old"),
        storage.join("sample--wt-young"),
        storage.join("sample-main"),
    );
    for dir in [&old, &young, &plain] {
        std::fs::create_dir(dir).unwrap();
        std::fs::write(dir.join(crate::workspace::prune::LAST_USED_MARKER), "").unwrap();
    }
    std::fs::File::open(old.join(crate::workspace::prune::LAST_USED_MARKER))
        .unwrap()
        .set_modified(std::time::SystemTime::UNIX_EPOCH)
        .unwrap();
    let state = Arc::new(state);
    // A floor of 100% is never met, so every copy idle for an hour goes, whatever the disk.
    let task = tokio::spawn(janitor_with_probe(
        Arc::clone(&state),
        0,
        0,
        0,
        100,
        Duration::from_millis(5),
        |_| prod_code_protocol::HostResources::default(),
    ));
    tokio::time::timeout(Duration::from_secs(10), async {
        while old.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(young.is_dir(), "a copy used a moment ago stays");
    assert!(plain.is_dir(), "a main workspace is not a worktree copy");
}

#[tokio::test]
async fn evacuation_moves_on_from_a_workspace_nobody_follows_and_never_targets_this_node() {
    crate::test_cache_support::isolated!();
    let temp = tempfile::tempdir().unwrap();
    let state = state_with_storage(temp.path());
    *state.advertise.write().await = "127.0.0.1:9400".into();
    add_peer(&state, "127.0.0.2:9400").await;
    let unwatched = load_active(&state, "unwatched").await;
    let watched = load_active(&state, "watched").await;
    let mut redirects = watched.rebalance_tx.subscribe();
    let state = Arc::new(state);
    let task = tokio::spawn(janitor_with_probe(
        Arc::clone(&state),
        0,
        0,
        0,
        0,
        Duration::from_millis(5),
        SHORT,
    ));
    let redirect = tokio::time::timeout(Duration::from_secs(10), redirects.recv())
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        redirect,
        (
            "127.0.0.2:9400".into(),
            Some("evacuating node under memory pressure".into())
        )
    );
    assert_eq!(unwatched.active_sessions.load(Ordering::Relaxed), 1);
    assert_eq!(watched.active_sessions.load(Ordering::Relaxed), 1);

    // The only compatible node is this one: nothing is redirected.
    let temp = tempfile::tempdir().unwrap();
    let state = state_with_storage(temp.path());
    *state.advertise.write().await = "127.0.0.2:9400".into();
    add_peer(&state, "127.0.0.2:9400").await;
    let workspace = load_active(&state, "alone").await;
    let mut redirects = workspace.rebalance_tx.subscribe();
    let state = Arc::new(state);
    let probes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&probes);
    let task = tokio::spawn(janitor_with_probe(
        Arc::clone(&state),
        0,
        0,
        0,
        0,
        Duration::from_millis(5),
        move |path| {
            counted.fetch_add(1, Ordering::Relaxed);
            SHORT(path)
        },
    ));
    tokio::time::timeout(Duration::from_secs(10), async {
        while probes.load(Ordering::Relaxed) < 4 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(redirects.try_recv().is_err());
}

#[tokio::test]
async fn the_toolchain_inventory_is_collected_again_on_every_sixtieth_sweep() {
    crate::test_cache_support::isolated!();
    let temp = tempfile::tempdir().unwrap();
    let state = state_with_storage(temp.path());
    state.metrics.set_node("sample");
    let state = Arc::new(state);
    let probes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&probes);
    let task = tokio::spawn(janitor_with_probe(
        Arc::clone(&state),
        0,
        0,
        0,
        0,
        Duration::from_millis(1),
        move |_| {
            counted.fetch_add(1, Ordering::Relaxed);
            prod_code_protocol::HostResources::default()
        },
    ));
    tokio::time::timeout(Duration::from_secs(60), async {
        while state.metrics.toolchain_inventory().unwrap().node == "preset" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    // The sixtieth sweep collects the inventory before it reads the host.
    assert!(probes.load(Ordering::Relaxed) >= 59);
    assert_eq!(state.metrics.toolchain_inventory().unwrap().node, "sample");
}
