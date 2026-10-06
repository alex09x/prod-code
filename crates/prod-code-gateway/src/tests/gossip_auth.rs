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
use futures_util::StreamExt;
use prod_code_protocol::{PROTOCOL_VERSION, WireMessage};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

#[tokio::test]
async fn own_gossip_does_not_propagate_unconfirmed_transitive_peers() {
    let storage = tempfile::tempdir().unwrap();
    let state = ServerState::new(storage.path().to_path_buf());
    *state.advertise.write().await = "127.0.0.1:9400".into();

    // Absorb gossip from peer A that mentions a transitive dead peer B.
    let peer_a_gossip = NodeGossip {
        addr: "127.0.0.1:9401".into(),
        status: state.status().await,
        workspaces: Vec::new(),
        peers: vec!["127.0.0.1:9402".into()], // peer B (unconfirmed)
        sent_at_ms: 1000,
    };
    state.absorb_gossip(peer_a_gossip).await;

    // own_gossip should contain peer A (which is confirmed alive by absorb_gossip),
    // but must NOT propagate peer B (which has never been directly observed alive).
    let own = state.own_gossip().await;
    assert!(
        own.peers.contains(&"127.0.0.1:9401".to_string()),
        "confirmed peer A must be gossiped"
    );
    assert!(
        !own.peers.contains(&"127.0.0.1:9402".to_string()),
        "unconfirmed peer B must NOT be propagated transitively"
    );
}

#[test]
fn shadow_root_ownership_follows_the_last_server_state_reference() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("shadow");
    let mut state = ServerState::new(fixture.path().join("storage"));
    state.shadow_root = root.clone();
    state._shadow_root_owner = Some(shadow::ShadowRootOwner::acquire(&root).unwrap());

    let accept_state = Arc::new(state);
    let session_state = Arc::clone(&accept_state);
    drop(accept_state);

    let refused = shadow::ShadowRootOwner::acquire(&root)
        .err()
        .expect("a retained session state must keep ownership");
    assert!(
        refused
            .to_string()
            .contains("already owned by another gateway"),
        "{refused:#}"
    );

    drop(session_state);
    let mut acquired = shadow::ShadowRootOwner::acquire(&root);
    for _ in 0..20 {
        if acquired.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        acquired = shadow::ShadowRootOwner::acquire(&root);
    }
    acquired.expect("the final state release must release ownership");
}

#[tokio::test]
async fn incompatible_protocol_offers_are_refused_before_session_or_workspace_side_effects() {
    for (name, protocol_version, supported_versions) in [
        ("empty", PROTOCOL_VERSION, Some(Vec::new())),
        ("disjoint", PROTOCOL_VERSION, Some(vec![2, 3])),
        ("legacy-unsupported", 999, None),
    ] {
        let storage = tempfile::tempdir().unwrap();
        let storage_root = storage.path().join("workspaces");
        let state = Arc::new(ServerState::new(storage_root.clone()));
        let client_root = format!("/home/dev/{name}");
        let server_root = workspace::server_workspace_path(&storage_root, &client_root, None);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let serving = Arc::clone(&state);
        let serve = tokio::spawn(async move {
            let (socket, peer) = listener.accept().await.unwrap();
            handle_client(socket, peer, serving).await
        });

        let mut stream = TcpStream::connect(addr).await.unwrap();
        let mut payload = serde_json::json!({
            "protocol_version": protocol_version,
            "client_name": format!("{name}-offer-test"),
            "client_pid": 1,
            "auth_token": null,
            "client_workspace_root": client_root
        });
        if let Some(versions) = supported_versions {
            payload["supported_versions"] = serde_json::json!(versions);
        }
        let request = serde_json::json!({
            "type": "HandshakeRequest",
            "payload": payload
        });
        let bytes = serde_json::to_vec(&request).unwrap();
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&bytes).await.unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());

        let Some(Ok(WireMessage::Disconnect { reason })) = framed.next().await else {
            panic!("the {name} protocol offer was not refused");
        };
        assert!(reason.contains("protocol"), "{reason}");
        drop(framed);
        serve.await.unwrap().unwrap();
        assert_eq!(state.next_session_id.load(Ordering::Relaxed), 1, "{name}");
        assert_eq!(state.active_sessions.load(Ordering::Relaxed), 0, "{name}");
        assert_eq!(state.workspace_manager.loaded_count().await, 0, "{name}");
        assert!(!server_root.exists(), "{name}");
    }
}

#[test]
fn native_position_coordinates_are_checked_before_one_based_conversion() {
    assert_eq!(
        one_based_position(Some(&serde_json::json!({ "line": 0, "character": 0 }))),
        Ok((1, 1))
    );
    assert_eq!(
        one_based_position(Some(&serde_json::json!({
            "line": 4_294_967_294u64,
            "character": 4_294_967_294u64,
        }))),
        Ok((u32::MAX, u32::MAX))
    );

    for malformed in [
        serde_json::json!({}),
        serde_json::json!({ "line": -1, "character": 0 }),
        serde_json::json!({ "line": 0.5, "character": 0 }),
        serde_json::json!({ "line": "0", "character": 0 }),
        serde_json::json!({ "line": null, "character": 0 }),
        serde_json::json!({ "line": 4_294_967_295u64, "character": 0 }),
    ] {
        assert!(one_based_position(Some(&malformed)).is_err(), "{malformed}");
    }
}

#[test]
fn native_methods_validate_positions_without_breaking_positionless_requests() {
    let point = serde_json::json!({ "line": 0, "character": 0 });
    assert!(
        native_position_params(
            Some("textDocument/hover"),
            Some(&serde_json::json!({ "position": point }))
        )
        .is_ok()
    );
    assert!(
        native_position_params(Some("textDocument/hover"), Some(&serde_json::json!({}))).is_err()
    );
    assert!(
        native_position_params(
            Some("callHierarchy/incomingCalls"),
            Some(&serde_json::json!({
                "item": { "selectionRange": { "start": { "line": -1, "character": 0 } } }
            }))
        )
        .is_err()
    );
    assert!(
        native_position_params(
            Some("prodCode/assists"),
            Some(&serde_json::json!({
                "range": { "start": { "line": 0, "character": 0 } }
            }))
        )
        .is_ok()
    );
    assert!(
        native_position_params(
            Some("prodCode/assists"),
            Some(&serde_json::json!({
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": "bad" }
                }
            }))
        )
        .is_err()
    );
    assert!(
        native_position_params(
            Some("prodCode/structuralReplace"),
            Some(&serde_json::json!({}))
        )
        .is_ok()
    );
    assert!(
        native_position_params(
            Some("prodCode/structuralReplace"),
            Some(&serde_json::json!({ "position": null }))
        )
        .is_err()
    );
    assert!(
        native_position_params(
            Some("textDocument/documentSymbol"),
            Some(&serde_json::json!({}))
        )
        .is_ok()
    );
    assert!(native_position_params(Some("workspace/symbol"), Some(&serde_json::json!({}))).is_ok());
    assert!(
        native_position_params(
            Some("textDocument/diagnostic"),
            Some(&serde_json::json!({}))
        )
        .is_ok()
    );
}
