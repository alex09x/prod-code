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
use crate::tests::common::peer;
use futures_util::StreamExt;
use prod_code_protocol::{ClientCapabilities, HostResources, LoadedWorkspaceInfo, NodeGossip};

fn request(redirects: bool, redirect_count: u32) -> HandshakeRequest {
    HandshakeRequest {
        protocol_version: prod_code_protocol::PROTOCOL_VERSION,
        supported_versions: None,
        capabilities: Some(ClientCapabilities {
            redirects,
            ..Default::default()
        }),
        client_name: "unit".to_string(),
        client_pid: 1,
        auth_token: None,
        client_workspace_root: "/client/unit-ws".to_string(),
        preferred_engine: None,
        base_workspace_name: Some("unit-ws".to_string()),
        engine_subpath: None,
        client_agent: None,
        client_host: None,
        purpose: None,
        redirect_count,
    }
}

/// A peer with plenty of memory and disk, so that nothing but the case under test excludes it.
fn roomy_peer(addr: &str, engines: &[&str], warm: &[&str]) -> NodeGossip {
    let gib = 1u64 << 30;
    let mut info = peer(addr, "linux x86_64", engines, 0.05);
    info.status.host = HostResources {
        memory_available_bytes: Some(50 * gib),
        memory_total_bytes: Some(100 * gib),
        storage_free_millis: Some(500),
    };
    NodeGossip {
        addr: addr.to_string(),
        status: info.status,
        workspaces: warm
            .iter()
            .map(|name| LoadedWorkspaceInfo {
                name: name.to_string(),
                engine: "rust".to_string(),
                sessions: 1,
            })
            .collect(),
        peers: Vec::new(),
        sent_at_ms: 0,
    }
}

/// What `check_cluster_redirects` answered and sent for a handshake on a node serving `serves`.
async fn check(
    req: HandshakeRequest,
    serves: &[&str],
    peers: Vec<NodeGossip>,
    engine: &str,
) -> (Option<()>, Option<WireMessage>) {
    let dir = tempfile::tempdir().unwrap();
    let mut state = ServerState::new(dir.path().join("storage"));
    state.engine_allowlist = serves.iter().map(ToString::to_string).collect();
    let state = Arc::new(state);
    for gossip in peers {
        state.absorb_gossip(gossip).await;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let mut framed = Framed::new(AnyStream::from(server), ProdCodeCodec::new());
    let workspace = dir.path().join("storage/unit-ws");
    let outcome = check_cluster_redirects(&req, &state, engine, &workspace, &mut framed)
        .await
        .unwrap();
    drop(framed);
    let sent = Framed::new(client, ProdCodeCodec::new()).next().await;
    (outcome, sent.map(|m| m.unwrap()))
}

#[tokio::test]
async fn an_engine_this_node_does_not_serve_is_redirected_or_refused() {
    let owner = || vec![roomy_peer("owner:9400", &["swift (sourcekit-lsp)"], &[])];
    let refused =
        "engine swift is not served by this node (--engines rust,go); pick a node that lists it";
    for (name, req, peers, expected) in [
        (
            "redirect to a capable node",
            request(true, 0),
            owner(),
            WireMessage::Redirect {
                target_addr: "owner:9400".to_string(),
                reason: Some("engine swift is served by owner:9400".to_string()),
            },
        ),
        (
            "second hop still allowed",
            request(true, 1),
            owner(),
            WireMessage::Redirect {
                target_addr: "owner:9400".to_string(),
                reason: Some("engine swift is served by owner:9400".to_string()),
            },
        ),
        (
            "client cannot follow redirects",
            request(false, 0),
            owner(),
            WireMessage::Disconnect {
                reason: refused.to_string(),
            },
        ),
        (
            "too many hops",
            request(true, 2),
            owner(),
            WireMessage::Disconnect {
                reason: refused.to_string(),
            },
        ),
        (
            "nobody serves it",
            request(true, 0),
            vec![roomy_peer("other:9400", &["rust (ra_ap_ide)"], &[])],
            WireMessage::Disconnect {
                reason: refused.to_string(),
            },
        ),
        (
            "empty cluster",
            request(true, 0),
            Vec::new(),
            WireMessage::Disconnect {
                reason: refused.to_string(),
            },
        ),
    ] {
        let (outcome, sent) = check(req, &["rust", "go"], peers, "swift").await;
        assert_eq!(outcome, Some(()), "{name}");
        assert_eq!(sent, Some(expected), "{name}");
    }
}

#[tokio::test]
async fn a_workspace_warm_on_another_node_sends_a_client_that_follows_redirects_there() {
    let warm = |addr: &str| roomy_peer(addr, &["rust (ra_ap_ide)"], &["unit-ws"]);
    let (outcome, sent) = check(request(true, 0), &[], vec![warm("holder:9400")], "rust").await;
    assert_eq!(outcome, Some(()));
    assert_eq!(
        sent,
        Some(WireMessage::Redirect {
            target_addr: "holder:9400".to_string(),
            reason: Some("workspace unit-ws is already warm on holder:9400".to_string()),
        })
    );
    // Not for a client that cannot follow redirects, and not to a peer that is short of memory
    // or does not serve the engine. (The node's own congestion, which depends on the machine
    // the test runs on, can still send a client to a roomy peer, so no such peer is used here.)
    let gib = 1u64 << 30;
    let mut short = warm("short:9400");
    short.status.host.memory_available_bytes = Some(5 * gib);
    let other_engine = roomy_peer("go-only:9400", &["go"], &["unit-ws"]);
    for (name, req, peers) in [
        (
            "no redirect support",
            request(false, 0),
            vec![warm("holder:9400")],
        ),
        ("peer short of memory", request(true, 0), vec![short]),
        (
            "peer without the engine",
            request(true, 0),
            vec![other_engine],
        ),
        ("no peers", request(true, 0), Vec::new()),
    ] {
        let (outcome, sent) = check(req, &[], peers, "rust").await;
        assert_eq!((outcome, sent), (None, None), "{name}");
    }
}

/// #1495: a validation handshake refused for capacity is sent to a roomy peer only when the
/// client follows redirects; an older client is refused with the reason, since it cannot read
/// the frame. A session moved by rebalancing tells such a client where to reconnect instead.
#[tokio::test]
async fn validation_and_rebalance_redirects_reach_only_clients_that_follow_them() {
    let dir = tempfile::tempdir().unwrap();
    let state = Arc::new(ServerState::new(dir.path().join("storage")));
    state
        .absorb_gossip(roomy_peer("roomy:9400", &["rust (ra_ap_ide)"], &[]))
        .await;
    let capacity = anyhow::anyhow!("capacity: this node has no memory for a new rust engine");
    let (state, capacity) = (&state, &capacity);
    let target = |req: HandshakeRequest| async move {
        validation_redirect_target(&req, state, "rust", capacity).await
    };
    assert_eq!(
        target(request(true, 0)).await.as_deref(),
        Some("roomy:9400")
    );
    assert_eq!(target(request(false, 0)).await, None, "no redirect support");
    let mut legacy = request(true, 0);
    legacy.capabilities = None;
    assert_eq!(target(legacy).await, None, "no capabilities at all");
    assert_eq!(target(request(true, 2)).await, None, "out of hops");
    let other = anyhow::anyhow!("the engine failed to load");
    assert_eq!(
        validation_redirect_target(&request(true, 0), state, "rust", &other).await,
        None,
        "not a capacity refusal"
    );

    // #1496: peers are ranked by what they would admit, not by their raw free memory. A large,
    // nearly full host (40 of 256 GiB free: 1.6 GiB under its 85% limit) loses to a small idle
    // one (20 of 32 GiB free: 15.2 GiB), and a cold peer needs room for the main engine too.
    let gib = 1u64 << 30;
    let sized = |addr: &str, available: u64, total: u64, warm: &[&str]| {
        let mut peer = roomy_peer(addr, &["rust (ra_ap_ide)"], warm);
        peer.status.host.memory_available_bytes = Some(available * gib);
        peer.status.host.memory_total_bytes = Some(total * gib);
        peer
    };
    for (name, peers, expected) in [
        (
            "admissible headroom, not free bytes",
            vec![
                sized("big:9400", 40, 256, &[]),
                sized("small:9400", 20, 32, &[]),
            ],
            Some("small:9400"),
        ),
        (
            // Warm: 5 GiB under the limit takes the 4 GiB validation engine. Cold: 7 GiB, and
            // more free memory, cannot take it and the 4 GiB main engine both.
            "a cold peer needs two engines' room",
            vec![
                sized("cold:9400", 22, 100, &[]),
                sized("warm:9400", 20, 100, &["unit-ws"]),
            ],
            Some("warm:9400"),
        ),
        (
            "no peer can admit it",
            vec![sized("full:9400", 40, 256, &[])],
            None,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(ServerState::new(dir.path().join("storage")));
        for gossip in peers {
            state.absorb_gossip(gossip).await;
        }
        let chosen = validation_redirect_target(&request(true, 0), &state, "rust", capacity).await;
        assert_eq!(chosen.as_deref(), expected, "{name}");
    }

    let reason = Some("memory pressure".to_string());
    assert_eq!(
        rebalance_frame(true, "roomy:9400".to_string(), reason.clone()),
        WireMessage::Redirect {
            target_addr: "roomy:9400".to_string(),
            reason: reason.clone(),
        }
    );
    assert_eq!(
        rebalance_frame(false, "roomy:9400".to_string(), reason),
        WireMessage::Disconnect {
            reason: "this workspace moved to roomy:9400 (memory pressure); reconnect there"
                .to_string(),
        }
    );
}
