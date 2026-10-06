/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::*;
use super::common::peer;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HostResources, LoadedWorkspaceInfo, PROTOCOL_VERSION, PlaceRequest, WireMessage,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

/// A node past 85% of its memory or under 10% of its disk gets no new workspace while
/// another capable node has room, however quiet it is, and gives up an idle one it holds;
/// one with a session stays, and when every node is short the quietest still takes it
/// (#396).
#[test]
fn a_node_short_of_memory_or_disk_takes_no_new_workspace() {
    let gib = 1 << 30;
    let short_of_disk = HostResources {
        memory_available_bytes: Some(50 * gib),
        memory_total_bytes: Some(100 * gib),
        storage_free_millis: Some(30),
    };
    let short_of_memory = HostResources {
        memory_available_bytes: Some(5 * gib),
        storage_free_millis: Some(500),
        ..short_of_disk.clone()
    };
    let roomy = HostResources {
        storage_free_millis: Some(500),
        ..short_of_disk.clone()
    };
    let mut quiet_but_full = peer("full:9400", "linux x86_64", &["rust (ra_ap_ide)"], 0.05);
    quiet_but_full.status.host = short_of_disk;
    let mut busier = peer("busy:9400", "linux x86_64", &["rust (ra_ap_ide)"], 0.6);
    busier.status.host = roomy;
    let view = ClusterResponse {
        this_node: "full:9400".to_string(),
        nodes: vec![quiet_but_full, busier],
    };
    let place = |view: &ClusterResponse| {
        place_in(
            &PlaceRequest {
                workspace_name: "subject".to_string(),
                engine: Some("rust".to_string()),
                os: None,
                rebalance_active: false,
            },
            view.clone(),
        )
    };

    let answer = place(&view);
    assert_eq!(
        answer.node.as_deref(),
        Some("busy:9400"),
        "{}",
        answer.reason
    );
    assert!(
        answer
            .reason
            .contains("passed over full:9400 (disk 3% free)"),
        "{}",
        answer.reason
    );

    let mut held = view.clone();
    held.nodes[0].workspaces.push(LoadedWorkspaceInfo {
        name: "subject".to_string(),
        engine: "rust".to_string(),
        sessions: 0,
    });
    let answer = place(&held);
    assert_eq!(
        answer.node.as_deref(),
        Some("busy:9400"),
        "{}",
        answer.reason
    );
    assert!(
        answer
            .reason
            .starts_with("moved from full:9400 (disk 3% free, idle)"),
        "{}",
        answer.reason
    );

    held.nodes[0].workspaces[0].sessions = 1;
    assert_eq!(
        place(&held).node.as_deref(),
        Some("full:9400"),
        "a workspace in use is not moved"
    );

    let rebalance_answer = place_in(
        &PlaceRequest {
            workspace_name: "subject".to_string(),
            engine: Some("rust".to_string()),
            os: None,
            rebalance_active: true,
        },
        held.clone(),
    );
    assert_eq!(
        rebalance_answer.node.as_deref(),
        Some("busy:9400"),
        "rebalance_active moves an active workspace off an overloaded node"
    );
    assert!(
        rebalance_answer.reason.contains("active, 1 sessions"),
        "reason explains session state: {}",
        rebalance_answer.reason
    );

    let mut all_short = view.clone();
    all_short.nodes[1].status.host = short_of_memory;
    let answer = place(&all_short);
    assert_eq!(
        answer.node.as_deref(),
        Some("full:9400"),
        "{}",
        answer.reason
    );
    assert!(
        answer.reason.contains("it is short too (disk 3% free)"),
        "{}",
        answer.reason
    );

    // A gateway too old to report its host is not taken for one that is short.
    let mut old = view.clone();
    old.nodes[1].status.host = HostResources::default();
    assert_eq!(place(&old).node.as_deref(), Some("busy:9400"));
}

#[test]
fn congested_node_rebalances_active_workspace_when_requested() {
    let gib = 1 << 30;
    let roomy = HostResources {
        memory_available_bytes: Some(50 * gib),
        memory_total_bytes: Some(100 * gib),
        storage_free_millis: Some(500),
    };
    let mut congested = peer("congested:9400", "linux x86_64", &["rust (ra_ap_ide)"], 1.5);
    congested.status.host = roomy.clone();
    congested.workspaces.push(LoadedWorkspaceInfo {
        name: "repo".to_string(),
        engine: "rust".to_string(),
        sessions: 2,
    });
    let mut quiet = peer("quiet:9400", "linux x86_64", &["rust (ra_ap_ide)"], 0.2);
    quiet.status.host = roomy;
    let view = ClusterResponse {
        this_node: "congested:9400".to_string(),
        nodes: vec![congested, quiet],
    };

    // Without rebalance_active, active sessions are not moved:
    let normal = place_in(
        &PlaceRequest {
            workspace_name: "repo".to_string(),
            engine: Some("rust".to_string()),
            os: None,
            rebalance_active: false,
        },
        view.clone(),
    );
    assert_eq!(normal.node.as_deref(), Some("congested:9400"));
    assert!(normal.reason.contains("already loaded"));

    // With rebalance_active, active sessions are moved to the quieter node:
    let rebalanced = place_in(
        &PlaceRequest {
            workspace_name: "repo".to_string(),
            engine: Some("rust".to_string()),
            os: None,
            rebalance_active: true,
        },
        view,
    );
    assert_eq!(rebalanced.node.as_deref(), Some("quiet:9400"));
    assert!(rebalanced.reason.contains("rebalanced from congested:9400"));
    assert!(rebalanced.reason.contains("active, 2 sessions"));
}

/// Idle engines go after `--idle-evict-secs`, or after five minutes while memory is short,
/// even when eviction is switched off (#396).
#[test]
fn a_host_short_of_memory_unloads_idle_engines_sooner() {
    assert_eq!(evict_after(1800, false), Some(Duration::from_secs(1800)));
    assert_eq!(evict_after(0, false), None);
    assert_eq!(evict_after(1800, true), Some(Duration::from_secs(300)));
    assert_eq!(evict_after(120, true), Some(Duration::from_secs(120)));
    assert_eq!(evict_after(0, true), Some(Duration::from_secs(300)));
}

/// A handshake that needs a new engine on a host without the memory for it is refused with
/// capacity as the reason and a way forward, and leaves no session counted (#433).
#[tokio::test]
async fn a_handshake_without_memory_for_its_engine_is_refused_for_capacity() {
    const GIB: u64 = 1 << 30;
    let storage = tempfile::tempdir().expect("tempdir");
    let mut state = ServerState::new(storage.path().join("workspaces"));
    state.workspace_manager = Arc::new(WorkspaceManager::with_admission(Arc::new(
        admission::Admission::with_probe(
            admission::scripted_probe(vec![(10 * GIB, 100 * GIB)]),
            2048,
            admission::LOAD_SETTLE,
        ),
    )));
    let client_root = "/home/dev/app";
    let server_root = workspace::resolve_server_workspace(&state.storage_root, client_root, None);
    std::fs::create_dir_all(&server_root).unwrap();
    let state = Arc::new(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = Arc::clone(&state);
    let serve = tokio::spawn(async move {
        let (socket, peer) = listener.accept().await.unwrap();
        handle_client(socket, peer, serving).await
    });
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::HandshakeRequest(
            prod_code_protocol::HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                supported_versions: Some(vec![PROTOCOL_VERSION]),
                capabilities: None,
                client_name: "test".to_string(),
                client_pid: 1,
                auth_token: None,
                client_workspace_root: client_root.to_string(),
                preferred_engine: None,
                base_workspace_name: None,
                engine_subpath: None,
                client_agent: None,
                client_host: None,
                purpose: None,
                redirect_count: 0,
            },
        ))
        .await
        .unwrap();
    let Some(Ok(WireMessage::Disconnect { reason })) = framed.next().await else {
        panic!("the handshake was not refused");
    };
    assert!(reason.starts_with("capacity: "), "{reason}");
    assert!(reason.contains("memory 90% used"), "{reason}");
    assert!(reason.contains("Retry in a few minutes"), "{reason}");
    assert!(reason.contains("another node"), "{reason}");
    serve.await.unwrap().unwrap();
    assert_eq!(state.active_sessions.load(Ordering::Relaxed), 0);
    assert_eq!(state.workspace_manager.loaded_count().await, 0);
}

/// The first answer `state` gives a connection that opens with `token`, if any, and asks for
/// the status.
async fn status_answer(state: Arc<ServerState>, token: Option<&str>) -> WireMessage {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve = tokio::spawn(async move {
        let (socket, peer) = listener.accept().await.unwrap();
        handle_client(socket, peer, state).await
    });
    let stream = prod_code_protocol::transport::connect_with(addr, token)
        .await
        .unwrap();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed.send(WireMessage::StatusRequest).await.unwrap();
    let answer = framed.next().await.unwrap().unwrap();
    drop(framed);
    let _ = serve.await;
    answer
}

/// A gateway with a token closes every connection that does not open with it and says
/// why, and serves one that does; a gateway without one ignores a token it is sent (#402).
#[tokio::test]
async fn a_gateway_with_a_token_serves_only_connections_that_open_with_it() {
    let storage = tempfile::tempdir().unwrap();
    let mut guarded = ServerState::new(storage.path().join("guarded"));
    guarded.auth_token = Some("s3cret".to_string());
    let guarded = Arc::new(guarded);
    for token in [None, Some("wrong"), Some("s3cre")] {
        match status_answer(Arc::clone(&guarded), token).await {
            WireMessage::Disconnect { reason } => {
                assert!(reason.contains("PROD_CODE_AUTH_TOKEN"), "{reason}")
            }
            other => panic!("served with {token:?}: {other:?}"),
        }
    }
    assert!(matches!(
        status_answer(Arc::clone(&guarded), Some("s3cret")).await,
        WireMessage::StatusResponse(_)
    ));

    let open = Arc::new(ServerState::new(storage.path().join("open")));
    assert!(matches!(
        status_answer(Arc::clone(&open), Some("anything")).await,
        WireMessage::StatusResponse(_)
    ));
    assert!(matches!(
        status_answer(open, None).await,
        WireMessage::StatusResponse(_)
    ));
}
