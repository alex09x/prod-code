/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Strict node pins on the gateway side (#1793). A client whose workspace is pinned to this node
//! asks, before it syncs or runs anything, that the connection's work stays here. The gateway
//! confirms with the address it advertises, so the client can name the node that executed, and
//! from then on serves the connection itself: a handshake on it is never redirected, whatever
//! the client offered.

use crate::*;
use prod_code_protocol::{HandshakeRequest, NodePinRequest, NodePinResponse};

/// The answer to a client's pin request. A request that names no endpoint is refused: the
/// client could not tell what it holds the node to.
pub(crate) async fn answer_pin(req: &NodePinRequest, state: &ServerState) -> NodePinResponse {
    if req.endpoint.trim().is_empty() {
        return NodePinResponse {
            accepted: false,
            node: None,
            reason: Some("the node pin names no endpoint".to_string()),
        };
    }
    let node = state.advertise.read().await.clone();
    tracing::info!(
        endpoint = %req.endpoint,
        connected_to = ?req.connected_to,
        node = %node,
        "connection pinned to this node"
    );
    NodePinResponse {
        accepted: true,
        node: (!node.is_empty()).then_some(node),
        reason: None,
    }
}

/// A handshake on a pinned connection is served or refused here: it is never redirected.
pub(crate) fn hold_handshake(req: &mut HandshakeRequest) {
    if let Some(capabilities) = req.capabilities.as_mut() {
        capabilities.redirects = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use std::sync::Arc;

    async fn serve() -> (
        tempfile::TempDir,
        Framed<tokio::net::TcpStream, ProdCodeCodec>,
        tokio::task::JoinHandle<Result<()>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path().join("storage");
        std::fs::create_dir_all(&storage).unwrap();
        let state = ServerState::new(storage);
        *state.advertise.write().await = "build.example:9400".to_string();
        let state = Arc::new(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let server = tokio::spawn(async move {
            let (socket, peer) = listener.accept().await.unwrap();
            crate::session::client_handler::handle_client(socket, peer, state).await
        });
        (dir, Framed::new(client, ProdCodeCodec::new()), server)
    }

    async fn ask(
        client: &mut Framed<tokio::net::TcpStream, ProdCodeCodec>,
        msg: WireMessage,
    ) -> WireMessage {
        client.send(msg).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(20), client.next())
            .await
            .expect("an answer")
            .expect("the connection stays open")
            .expect("a frame")
    }

    #[tokio::test]
    async fn issue_1793_gateway_confirms_a_pin_with_the_node_it_advertises() {
        let (_dir, mut client, server) = serve().await;
        let answer = ask(
            &mut client,
            WireMessage::NodePinRequest(NodePinRequest {
                endpoint: "build.example:9400".into(),
                connected_to: Some("127.0.0.1:9400".into()),
                client_workspace_root: "/checkout/app".into(),
            }),
        )
        .await;
        assert_eq!(
            answer,
            WireMessage::NodePinResponse(NodePinResponse {
                accepted: true,
                node: Some("build.example:9400".into()),
                reason: None,
            })
        );
        // The pinned connection goes on serving the requests that follow.
        let status = ask(&mut client, WireMessage::StatusRequest).await;
        assert!(
            matches!(status, WireMessage::StatusResponse(_)),
            "{status:?}"
        );
        client
            .send(WireMessage::Disconnect {
                reason: "done".into(),
            })
            .await
            .unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn issue_1793_gateway_refuses_a_pin_without_an_endpoint() {
        let (_dir, mut client, server) = serve().await;
        let answer = ask(
            &mut client,
            WireMessage::NodePinRequest(NodePinRequest::default()),
        )
        .await;
        let WireMessage::NodePinResponse(answer) = answer else {
            panic!("unexpected answer {answer:?}");
        };
        assert!(!answer.accepted);
        assert!(answer.reason.unwrap().contains("no endpoint"));
        server.abort();
    }

    #[test]
    fn issue_1793_a_pinned_handshake_is_never_redirected() {
        let mut req: HandshakeRequest = serde_json::from_value(serde_json::json!({
            "protocol_version": 1,
            "capabilities": { "redirects": true },
            "client_name": "test",
            "client_pid": 1,
            "auth_token": null,
            "client_workspace_root": "/checkout/app",
        }))
        .unwrap();
        hold_handshake(&mut req);
        assert!(!req.capabilities.unwrap().redirects);
    }
}
