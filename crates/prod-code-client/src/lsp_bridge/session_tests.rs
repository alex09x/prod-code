/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::test_support::handshake;
use super::*;

#[tokio::test]
async fn session_handshake_refusals_are_actionable() {
    for (response, expected) in [
        (
            Some(WireMessage::Disconnect {
                reason: "fixture refusal".into(),
            }),
            "the gateway refused the session: fixture refusal",
        ),
        (
            Some(WireMessage::Pong),
            "Expected HandshakeResponse, got Pong",
        ),
        (None, "Server closed connection during handshake"),
        (
            Some(WireMessage::Redirect {
                target_addr: "127.0.0.1:65536".into(),
                reason: None,
            }),
            "invalid redirect target address: 127.0.0.1:65536",
        ),
        (
            Some(WireMessage::Redirect {
                target_addr: "127.0.0.1:1".into(),
                reason: Some("loop".into()),
            }),
            "refusing gateway redirect without server-authenticated TLS; configure a trusted CA or certificate pin",
        ),
    ] {
        let ws = prod_code_testkit::Workspace::new(&[
            (
                "Cargo.toml",
                "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\n",
            ),
            ("src/lib.rs", "pub fn sample() {}\n"),
        ]);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut framed = Framed::new(socket, ProdCodeCodec::new());
            let request = handshake(&mut framed).await;
            assert_eq!(request.preferred_engine.as_deref(), Some("rust"));
            assert_eq!(
                request.purpose.as_deref(),
                Some(prod_code_protocol::PURPOSE_EDITOR)
            );
            assert_eq!(request.redirect_count, 0);
            if let Some(response) = response {
                framed.send(response).await.unwrap();
            }
        });
        let error = open_editor_session(
            remote,
            Some("rust"),
            &ws.root(),
            ws.root().display().to_string(),
            prod_code_mcp::sync::workspace_identity(&ws.root()),
            0,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), expected);
        peer.await.unwrap();
    }
}

#[test]
fn redirect_resolution_keeps_explicit_ports() {
    assert_eq!(
        resolve_redirect_target("127.0.0.1:1234").unwrap(),
        "127.0.0.1:1234".parse().unwrap()
    );
    assert_eq!(
        resolve_redirect_target("[::1]:1234").unwrap(),
        "[::1]:1234".parse().unwrap()
    );
}

#[tokio::test]
async fn session_reports_preflight_refusals_and_corrupt_handshake_frames() {
    use tokio::io::AsyncWriteExt;
    for corrupt in [false, true] {
        let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn sample() {}\n")]);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut framed = Framed::new(socket, ProdCodeCodec::new());
            if corrupt {
                handshake(&mut framed).await;
                let mut socket = framed.into_inner();
                socket.write_all(&[0, 0, 0, 1, 255]).await.unwrap();
            } else {
                assert!(matches!(
                    framed.next().await,
                    Some(Ok(WireMessage::SyncProbeRequest(_)))
                ));
                framed
                    .send(WireMessage::Disconnect {
                        reason: "fixture preflight refusal".into(),
                    })
                    .await
                    .unwrap();
            }
        });
        let error = open_editor_session(
            remote,
            None,
            &ws.root(),
            ws.root().display().to_string(),
            prod_code_mcp::sync::workspace_identity(&ws.root()),
            0,
        )
        .await
        .unwrap_err();
        if corrupt {
            assert_eq!(error.to_string(), "Unexpected EOF with 4 unparsed bytes");
        } else {
            assert_eq!(
                error.to_string(),
                "workspace sync before the language server session failed"
            );
        }
        peer.await.unwrap();
    }
}
