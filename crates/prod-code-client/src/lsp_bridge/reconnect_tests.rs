/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::test_support::socket_pair;
use super::super::transport::MAX_DEFERRED_EDITOR_FRAMES;
use super::*;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::Arc;

#[test]
fn a_workspace_model_change_reconnects_even_when_reconnect_is_disabled() {
    let reason = format!(
        "closed the session: {}",
        prod_code_protocol::WORKSPACE_MODEL_CHANGED_REASON
    );
    assert!(should_reconnect_after_close(false, false, &reason));
    assert!(!should_reconnect_after_close(
        false,
        false,
        "closed the session: eof"
    ));
    assert!(should_reconnect_after_close(
        true,
        false,
        "closed the session: eof"
    ));
    assert!(should_reconnect_after_close(
        false,
        true,
        "closed the session: eof"
    ));
}

async fn replay_case(
    init: Option<&str>,
    frames: Vec<std::result::Result<Option<String>, String>>,
    response: Option<WireMessage>,
    queued: usize,
    initialized: bool,
    expect_original_encoding: bool,
) -> (Result<Option<String>>, VecDeque<String>, u8) {
    let (socket, mut peer) = socket_pair().await;
    let (mut tx, mut rx) = socket.split();
    let temp = tempfile::tempdir().unwrap();
    let files = prod_code_client::editor_files::RemoteFiles::new(
        "127.0.0.1:1".parse().unwrap(),
        temp.path(),
        temp.path(),
        temp.path(),
    );
    let tracker = LspStateTracker {
        initialize_req: init.map(str::to_string),
        initialized_sent: initialized,
        ..Default::default()
    };
    let encoding = AtomicU8::new(0);
    let (sender, receiver) = tokio::sync::mpsc::channel(16);
    for frame in frames {
        sender.send(frame).await.unwrap();
    }
    let frames = Arc::new(tokio::sync::Mutex::new(receiver));
    let peer_task = tokio::spawn(async move {
        if let Some(Ok(WireMessage::LspPayload(raw))) = peer.next().await {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&raw).unwrap()["method"],
                "initialize"
            );
            if let Some(response) = response {
                peer.send(response).await.unwrap();
            }
        }
    });
    let mut deferred = VecDeque::from(vec!["queued".to_string(); queued]);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        replay_lsp_state(
            &mut tx,
            &mut rx,
            &tracker,
            expect_original_encoding,
            &files,
            &encoding,
            &frames,
            &mut deferred,
            &tokio::sync::Mutex::new(tokio::io::stdout()),
            &None,
        ),
    )
    .await
    .expect("bounded replay");
    drop(tx);
    drop(rx);
    peer_task.await.unwrap();
    (result, deferred, encoding.load(Ordering::Acquire))
}

#[tokio::test]
async fn replay_rejects_missing_initialize_id_and_unexpected_wire_messages() {
    let (result, _, _) = replay_case(
        Some(r#"{"method":"initialize"}"#),
        vec![],
        None,
        0,
        false,
        false,
    )
    .await;
    assert_eq!(
        result.unwrap_err().to_string(),
        "recorded initialize request has no JSON-RPC id"
    );
    let init = r#"{"id":1,"method":"initialize"}"#;
    for (message, expected) in [
        (
            Some(WireMessage::Ping),
            "unexpected message during initialize replay: Ping",
        ),
        (
            None,
            "connection closed while awaiting initialize replay response",
        ),
    ] {
        let (result, _, _) = replay_case(Some(init), vec![], message, 0, false, false).await;
        assert_eq!(result.unwrap_err().to_string(), expected);
    }
}

#[tokio::test]
async fn replay_reports_editor_read_failure_eof_and_queue_overflow() {
    let init = r#"{"id":1,"method":"initialize"}"#;
    // Keep the peer response readable but irrelevant until the editor error is consumed.
    for (frame, queued, expected) in [
        (
            Err("fixture read error".into()),
            0,
            "reading editor message during reconnect: fixture read error",
        ),
        (
            Ok(None),
            0,
            "editor closed during language-server reconnect",
        ),
        (
            Ok(Some(
                json!({"id":2,"method":"textDocument/hover"}).to_string(),
            )),
            MAX_DEFERRED_EDITOR_FRAMES,
            "too many editor messages queued during language-server reconnect",
        ),
    ] {
        let (socket, _peer) = socket_pair().await;
        let (mut tx, mut rx) = socket.split();
        let temp = tempfile::tempdir().unwrap();
        let files = prod_code_client::editor_files::RemoteFiles::new(
            "127.0.0.1:1".parse().unwrap(),
            temp.path(),
            temp.path(),
            temp.path(),
        );
        let tracker = LspStateTracker {
            initialize_req: Some(init.into()),
            ..Default::default()
        };
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        sender.send(frame).await.unwrap();
        let mut deferred = VecDeque::from(vec!["queued".to_string(); queued]);
        let error = replay_lsp_state(
            &mut tx,
            &mut rx,
            &tracker,
            false,
            &files,
            &AtomicU8::new(0),
            &Arc::new(tokio::sync::Mutex::new(receiver)),
            &mut deferred,
            &tokio::sync::Mutex::new(tokio::io::stdout()),
            &None,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(deferred.len(), queued);
    }
}

#[tokio::test]
async fn replay_returns_the_matching_initialize_response_and_updates_encoding() {
    let response =
        json!({"id":1,"result":{"capabilities":{"positionEncoding":"utf-8"}}}).to_string();
    let (result, deferred, encoding) = replay_case(
        Some(r#"{"id":1,"method":"initialize"}"#),
        vec![],
        Some(WireMessage::LspPayload(response.clone())),
        0,
        false,
        false,
    )
    .await;
    assert_eq!(result.unwrap(), Some(response));
    assert_eq!(encoding, 1);
    assert!(deferred.is_empty());
}

#[tokio::test]
async fn replay_rejects_a_changed_position_encoding_after_initialization() {
    let response =
        json!({"id":1,"result":{"capabilities":{"positionEncoding":"utf-8"}}}).to_string();
    let (result, _, encoding) = replay_case(
        Some(r#"{"id":1,"method":"initialize"}"#),
        vec![],
        Some(WireMessage::LspPayload(response)),
        0,
        true,
        true,
    )
    .await;
    assert!(result.unwrap_err().is::<IncompatiblePositionEncoding>());
    assert_eq!(encoding, POSITION_ENCODING_UTF16);
}

#[tokio::test]
async fn replay_preserves_encoding_even_before_initialized_notification_arrives() {
    let response =
        json!({"id":1,"result":{"capabilities":{"positionEncoding":"utf-8"}}}).to_string();
    let (result, _, encoding) = replay_case(
        Some(r#"{"id":1,"method":"initialize"}"#),
        vec![],
        Some(WireMessage::LspPayload(response)),
        0,
        false,
        true,
    )
    .await;
    assert!(result.unwrap_err().is::<IncompatiblePositionEncoding>());
    assert_eq!(encoding, POSITION_ENCODING_UTF16);
}

#[tokio::test]
async fn replay_answers_server_requests_and_defers_unrelated_editor_messages() {
    let (socket, mut peer) = socket_pair().await;
    let (mut tx, mut rx) = socket.split();
    let temp = tempfile::tempdir().unwrap();
    let files = prod_code_client::editor_files::RemoteFiles::new(
        "127.0.0.1:1".parse().unwrap(),
        temp.path(),
        temp.path(),
        temp.path(),
    );
    let log = tempfile::NamedTempFile::new().unwrap();
    let trace = Some(Arc::new(std::sync::Mutex::new(log.reopen().unwrap())));
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    let editor_frames = Arc::new(tokio::sync::Mutex::new(receiver));
    let initialize = json!({"id":1,"method":"initialize","params":{}}).to_string();
    let response =
        json!({"id":1,"result":{"capabilities":{"positionEncoding":"utf-16"}}}).to_string();
    let server_request =
        json!({"id":99,"method":"workspace/configuration","params":{"items":[]}}).to_string();
    let deferred_message = json!({"id":2,"method":"textDocument/hover","params":{}}).to_string();
    let editor_response = json!({"id":99,"result":[]}).to_string();
    let expected_editor_response = editor_response.clone();
    let response_copy = response.clone();
    let peer_task = tokio::spawn(async move {
        assert!(matches!(
            peer.next().await,
            Some(Ok(WireMessage::LspPayload(_)))
        ));
        peer.send(WireMessage::Pong).await.unwrap();
        peer.send(WireMessage::LspPayload(server_request))
            .await
            .unwrap();
        peer.send(WireMessage::LspPayload(response_copy))
            .await
            .unwrap();
        let WireMessage::LspPayload(raw) = peer.next().await.unwrap().unwrap() else {
            panic!("editor response expected")
        };
        assert_eq!(raw, expected_editor_response);
    });
    let log_path = log.path().to_path_buf();
    let deferred_copy = deferred_message.clone();
    let editor_task = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !std::fs::read_to_string(&log_path)
                .unwrap()
                .contains("workspace/configuration")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        sender.send(Ok(Some(deferred_copy))).await.unwrap();
        sender.send(Ok(Some(editor_response))).await.unwrap();
        sender
    });
    let tracker = LspStateTracker {
        initialize_req: Some(initialize),
        ..Default::default()
    };
    let mut deferred = VecDeque::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        replay_lsp_state(
            &mut tx,
            &mut rx,
            &tracker,
            false,
            &files,
            &AtomicU8::new(2),
            &editor_frames,
            &mut deferred,
            &tokio::sync::Mutex::new(tokio::io::stdout()),
            &trace,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result, Some(response));
    assert_eq!(deferred, VecDeque::from([deferred_message]));
    peer_task.await.unwrap();
    drop(editor_task.await.unwrap());
}
