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
use futures_util::{SinkExt, StreamExt};

#[test]
fn trace_records_method_id_direction_and_utf8_byte_length() {
    let temp = tempfile::NamedTempFile::new().unwrap();
    let trace = Some(Arc::new(std::sync::Mutex::new(temp.reopen().unwrap())));
    for (raw, suffix) in [
        (
            r#"{"id":7,"method":"sample","params":"界"}"#,
            " -> sample id=7 41B\n",
        ),
        ("bad", " ->  id= 3B\n"),
        (r#"{"id":"name","result":null}"#, " ->  id=\"name\" 27B\n"),
    ] {
        let before = std::fs::read_to_string(temp.path()).unwrap().len();
        trace_message(&trace, "->", raw);
        let content = std::fs::read_to_string(temp.path()).unwrap();
        let line = &content[before..];
        let (timestamp, actual) = line.split_once(' ').unwrap();
        assert!(timestamp.parse::<u128>().unwrap() > 0);
        assert_eq!(format!(" {actual}"), suffix);
    }
    trace_message(&None, "->", "unused");
}

#[tokio::test]
async fn stdout_task_tracks_encoding_pending_requests_pongs_and_disconnect() {
    let (socket, mut peer) = super::super::test_support::socket_pair().await;
    let (_tx, rx) = socket.split();
    let temp = tempfile::tempdir().unwrap();
    let files = Arc::new(prod_code_client::editor_files::RemoteFiles::new(
        "127.0.0.1:1".parse().unwrap(),
        temp.path(),
        temp.path(),
        temp.path(),
    ));
    let pending: PendingRequests = Arc::new(tokio::sync::Mutex::new(vec![
        (serde_json::json!(7), "initialize".into(), String::new()),
        (serde_json::json!(8), "hover".into(), String::new()),
    ]));
    let ping = Arc::new(AtomicBool::new(true));
    let encoding = Arc::new(AtomicU8::new(0));
    let (task, closed) = spawn_editor_stdout_task(
        rx,
        files,
        None,
        prod_code_mcp::sync::WorkspaceIdentity {
            name: "fixture-stdout".into(),
            base: None,
        },
        Arc::new(tokio::sync::Mutex::new(tokio::io::stdout())),
        pending.clone(),
        ping.clone(),
        encoding.clone(),
    );
    peer.send(WireMessage::Pong).await.unwrap();
    peer.send(WireMessage::Ping).await.unwrap();
    peer.send(WireMessage::LspPayload(
        serde_json::json!({"id":7,"result":{"capabilities":{"positionEncoding":"utf-32"}}})
            .to_string(),
    ))
    .await
    .unwrap();
    peer.send(WireMessage::Disconnect {
        reason: "fixture done".into(),
    })
    .await
    .unwrap();
    assert_eq!(
        closed.await.unwrap(),
        ("closed the session: fixture done".into(), None)
    );
    task.await.unwrap();
    assert_eq!(encoding.load(Ordering::Acquire) & 3, 2);
    assert!(!ping.load(Ordering::Acquire));
    assert_eq!(
        *pending.lock().await,
        vec![(serde_json::json!(8), "hover".into(), String::new())]
    );
}

#[tokio::test]
async fn stdout_task_reports_eof_after_a_redirect() {
    let (socket, mut peer) = super::super::test_support::socket_pair().await;
    let (_tx, rx) = socket.split();
    let temp = tempfile::tempdir().unwrap();
    let remote = "127.0.0.1:12345".parse().unwrap();
    let files = Arc::new(prod_code_client::editor_files::RemoteFiles::new(
        remote,
        temp.path(),
        temp.path(),
        temp.path(),
    ));
    let identity = prod_code_mcp::sync::WorkspaceIdentity {
        name: format!(
            "fixture-redirect-{}",
            temp.path().file_name().unwrap().to_string_lossy()
        ),
        base: None,
    };
    let (task, closed) = spawn_editor_stdout_task(
        rx,
        files,
        None,
        identity,
        Arc::new(tokio::sync::Mutex::new(tokio::io::stdout())),
        Arc::new(tokio::sync::Mutex::new(vec![])),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicU8::new(0)),
    );
    peer.send(WireMessage::Redirect {
        target_addr: remote.to_string(),
        reason: None,
    })
    .await
    .unwrap();
    drop(peer);
    assert_eq!(
        closed.await.unwrap(),
        ("closed the connection".into(), Some(remote))
    );
    task.await.unwrap();
}
