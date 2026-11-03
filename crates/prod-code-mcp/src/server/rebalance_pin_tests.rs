/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The background rebalance of a pinned workspace (#1793).

use super::rebalance::rebalance_tick_policy;
use crate::cluster::{Endpoint, NodePolicy, PinMode};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn cargo_checkout() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"pinned\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    dir
}

async fn tick(policy: &NodePolicy, remote: SocketAddr, better: SocketAddr) -> (SocketAddr, usize) {
    let root = cargo_checkout();
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = asked.clone();
    let mut remote = remote;
    rebalance_tick_policy(&mut remote, root.path(), policy, move |_, _, _, _| {
        counted.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Some((better, "roomier".to_string())))
    })
    .await;
    (remote, asked.load(Ordering::SeqCst))
}

#[tokio::test]
async fn issue_1793_rebalance_never_moves_or_asks_for_a_pinned_workspace() {
    // The pinned node does not answer, which unpinned would move away from.
    let pinned: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let better = listener.local_addr().unwrap();
    for mode in [PinMode::Strict, PinMode::Listed] {
        let policy = NodePolicy {
            mode,
            endpoints: vec![Endpoint {
                name: pinned.to_string(),
                addrs: vec![pinned],
            }],
            source: "--pin".into(),
        };
        let (remote, asked) = tick(&policy, pinned, better).await;
        assert_eq!((remote, asked), (pinned, 0), "{mode:?}");
    }
    // No pre-warm sync reached the node the cluster would have chosen.
    let warmed =
        tokio::time::timeout(std::time::Duration::from_millis(300), listener.accept()).await;
    assert!(warmed.is_err(), "nothing connected to the unapproved node");

    // Unpinned keeps moving to the better node, as before.
    let (remote, asked) = tick(&NodePolicy::default(), pinned, better).await;
    assert_eq!((remote, asked), (better, 1));
}
