/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::session::{close_session, initial_sync_with_timeout, open_session_with_timeout};
use super::super::setup::setup;
use super::super::types::{DivergentWorktree, WorkspaceMode, WorktreeKind};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeRequest, HandshakeResponse, PROTOCOL_VERSION, ProdCodeCodec, SyncProbeResponse,
    SyncResponse, WireMessage, supported_protocol_versions,
};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_util::codec::Framed;

fn fixture_worktree() -> (tempfile::TempDir, DivergentWorktree) {
    let tmp = tempfile::tempdir().expect("temporary fixture directory");
    let setup =
        setup(None, tmp.path(), WorkspaceMode::Shared, 1).expect("fixture setup should succeed");
    let worktree = setup
        .worktrees
        .into_iter()
        .find(|wt| wt.kind == WorktreeKind::Master)
        .expect("fixture has master worktree");
    (tmp, worktree)
}

async fn receive_sync_then_handshake(
    framed: &mut Framed<tokio::net::TcpStream, ProdCodeCodec>,
) -> HandshakeRequest {
    loop {
        match framed
            .next()
            .await
            .expect("client should keep the setup transport open")
            .expect("client setup frame should decode")
        {
            WireMessage::SyncProbeRequest(req) => {
                framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing: Vec::new(),
                    }))
                    .await
                    .expect("reply to sync probe");
            }
            WireMessage::SyncRequest(req) => {
                framed
                    .send(WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: req.client_workspace_root,
                        files_updated: 0,
                        files_deleted: 0,
                        bytes_transferred: 0,
                        duration_ms: 1,
                        workspace_was_fresh: false,
                        stale_paths: Vec::new(),
                    }))
                    .await
                    .expect("reply to sync request");
            }
            WireMessage::HandshakeRequest(req) => return req,
            other => panic!("unexpected setup frame: {other:?}"),
        }
    }
}

fn handshake_response(req: HandshakeRequest) -> HandshakeResponse {
    HandshakeResponse {
        protocol_version: PROTOCOL_VERSION,
        server_pid: std::process::id(),
        session_id: 1,
        server_workspace_root: req.client_workspace_root,
        detected_engine: "rust".to_string(),
        stale_paths: Vec::new(),
        engine_age_ms: None,
        index_gated: false,
        capabilities: None,
    }
}

fn assert_lsp_method(message: WireMessage, expected_method: &str) {
    let WireMessage::LspPayload(payload) = message else {
        panic!("expected LSP payload, got {message:?}");
    };
    let payload: serde_json::Value = serde_json::from_str(&payload).expect("valid LSP JSON");
    assert_eq!(
        payload.get("method").and_then(|m| m.as_str()),
        Some(expected_method)
    );
}

#[tokio::test]
async fn initial_sync_deadline_closes_a_stalled_origin_transport() {
    let (_tmp, worktree) = fixture_worktree();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
    let remote = listener.local_addr().expect("peer address");
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept client");
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        assert!(matches!(
            framed.next().await,
            Some(Ok(WireMessage::SyncProbeRequest(_)))
        ));
        assert!(
            framed.next().await.is_none(),
            "expired origin sync must close its transport"
        );
    });
    let result = timeout(
        Duration::from_secs(3),
        initial_sync_with_timeout(
            remote,
            &worktree.root,
            &worktree.workspace_name,
            Duration::from_millis(500),
        ),
    )
    .await
    .expect("the initial sync deadline must finish before the outer assertion");
    let error = result.expect_err("silent initial sync must fail");
    assert!(
        error
            .to_string()
            .contains("completing benchmark initial workspace sync"),
        "{error:#}"
    );
    timeout(Duration::from_secs(3), peer)
        .await
        .expect("peer must observe closure")
        .expect("peer completes");
}

#[tokio::test]
async fn open_session_completes_normal_sync_handshake_and_initialize() {
    let (_tmp, worktree) = fixture_worktree();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
    let remote = listener.local_addr().expect("peer address");
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept client");
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        let request = receive_sync_then_handshake(&mut framed).await;
        framed
            .send(WireMessage::HandshakeResponse(handshake_response(request)))
            .await
            .expect("reply to handshake");
        assert_lsp_method(
            framed
                .next()
                .await
                .expect("initialize frame")
                .expect("initialize decode"),
            "initialize",
        );
        framed
            .send(WireMessage::LspPayload(
                serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": {} }).to_string(),
            ))
            .await
            .expect("reply to initialize");
        assert_lsp_method(
            framed
                .next()
                .await
                .expect("initialized frame")
                .expect("initialized decode"),
            "initialized",
        );
        assert!(matches!(
            framed.next().await,
            Some(Ok(WireMessage::Disconnect { .. }))
        ));
    });

    let session = open_session_with_timeout(
        remote,
        &worktree,
        "divergent-bench-normal-setup".to_string(),
        Duration::from_secs(3),
    )
    .await
    .expect("normal setup succeeds");
    close_session(session).await;
    peer.await.expect("peer task completes");
}

#[tokio::test]
async fn open_session_times_out_after_silent_handshake_and_closes_transport() {
    let (_tmp, worktree) = fixture_worktree();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
    let remote = listener.local_addr().expect("peer address");
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept client");
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        let _request = receive_sync_then_handshake(&mut framed).await;
        assert!(
            framed.next().await.is_none(),
            "timed-out setup must drop its owned transport"
        );
    });

    let result = timeout(
        Duration::from_secs(3),
        open_session_with_timeout(
            remote,
            &worktree,
            "divergent-bench-silent-handshake".to_string(),
            Duration::from_millis(500),
        ),
    )
    .await
    .expect("the internal setup deadline must finish before this outer assertion");
    let error = result.expect_err("silent handshake must fail");
    assert!(
        error
            .to_string()
            .contains("completing gateway session setup"),
        "{error:#}"
    );
    peer.await.expect("peer observes connection closure");
}

#[tokio::test]
async fn open_session_timeout_covers_a_stalled_preflight_sync() {
    let (_tmp, worktree) = fixture_worktree();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
    let remote = listener.local_addr().expect("peer address");
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept client");
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        assert!(matches!(
            framed.next().await,
            Some(Ok(WireMessage::SyncProbeRequest(_)))
        ));
        assert!(
            framed.next().await.is_none(),
            "timed-out pre-flight sync must close its transport"
        );
    });

    let result = timeout(
        Duration::from_secs(3),
        open_session_with_timeout(
            remote,
            &worktree,
            "divergent-bench-stalled-preflight".to_string(),
            Duration::from_millis(500),
        ),
    )
    .await
    .expect("the internal setup deadline must bound pre-flight sync");
    let error = result.expect_err("stalled pre-flight sync must fail");
    assert!(
        error
            .to_string()
            .contains("completing gateway session setup")
    );
    peer.await.expect("peer observes connection closure");
}

#[tokio::test]
async fn open_session_keeps_unexpected_handshake_response_errors_contextual() {
    let (_tmp, worktree) = fixture_worktree();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
    let remote = listener.local_addr().expect("peer address");
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept client");
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        let _request = receive_sync_then_handshake(&mut framed).await;
        framed
            .send(WireMessage::Pong)
            .await
            .expect("send invalid reply");
    });

    let error = open_session_with_timeout(
        remote,
        &worktree,
        "divergent-bench-bad-handshake".to_string(),
        Duration::from_secs(3),
    )
    .await
    .expect_err("unexpected handshake reply must fail");
    assert!(error.to_string().contains("unexpected handshake response"));
    peer.await.expect("peer task completes");
}

#[tokio::test]
async fn open_session_refuses_a_bad_selected_version_before_initialize() {
    let (_tmp, worktree) = fixture_worktree();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
    let remote = listener.local_addr().expect("peer address");
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept client");
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        let request = receive_sync_then_handshake(&mut framed).await;
        assert_eq!(
            request.supported_versions,
            Some(supported_protocol_versions())
        );
        let mut response = handshake_response(request);
        response.protocol_version = 2;
        framed
            .send(WireMessage::HandshakeResponse(response))
            .await
            .expect("reply to handshake");
        assert!(
            framed.next().await.is_none(),
            "benchmark session must close before initialize"
        );
    });

    let error = open_session_with_timeout(
        remote,
        &worktree,
        "divergent-bench-bad-version".to_string(),
        Duration::from_secs(3),
    )
    .await
    .expect_err("incompatible selection must fail");
    assert!(
        error
            .to_string()
            .contains("incompatible benchmark handshake response"),
        "{error:#}"
    );
    peer.await.expect("peer task completes");
}
