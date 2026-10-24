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
use crate::config::GenericLspConfig;
use serde_json::json;
use std::time::Duration;

async fn engine(root: &std::path::Path) -> GenericLspEngine {
    let script = r#"import json,sys
while True:
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if not line.strip(): break
        if line.lower().startswith(b'content-length:'): length = int(line.split(b':')[1])
    request = json.loads(sys.stdin.buffer.read(length))
    if 'id' in request:
        body = json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'capabilities':{}}}).encode()
        sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n'%len(body)+body)
        sys.stdout.buffer.flush()
"#;
    GenericLspEngine::spawn(
        root,
        GenericLspConfig {
            command: "python3".into(),
            args: vec!["-c".into(), script.into()],
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn notification_records_versions_and_close_removes_sent_text() {
    let temp = tempfile::tempdir().unwrap();
    let engine = engine(temp.path()).await;
    let uri = "file:///example.test";
    for (method, version) in [
        ("textDocument/didOpen", Some(1)),
        ("textDocument/didChange", Some(2)),
        ("textDocument/didChange", None),
    ] {
        let mut params = json!({"textDocument":{"uri":uri}});
        if let Some(version) = version {
            params["textDocument"]["version"] = json!(version);
        }
        engine
            .record_and_write_notification_until(
                method,
                params,
                tokio::time::Instant::now() + Duration::from_secs(1),
            )
            .await
            .unwrap();
        assert_eq!(engine.sent.read().await[uri].version, version);
    }
    engine
        .record_and_write_notification_until(
            "custom",
            json!({"textDocument":{"uri":uri}}),
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(engine.sent.read().await.len(), 1);
    engine
        .record_and_write_notification_until(
            "textDocument/didClose",
            json!({"textDocument":{"uri":uri}}),
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert!(engine.sent.read().await.is_empty());
    engine
        .record_and_write_notification_until(
            "custom",
            json!({}),
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert!(engine.sent.read().await.is_empty());
}

#[tokio::test]
async fn notification_deadlines_identify_blocked_state_lock() {
    let temp = tempfile::tempdir().unwrap();
    let engine = engine(temp.path()).await;
    let params = json!({"textDocument":{"uri":"file:///example.test","version":1}});
    let diagnostics = engine.diagnostics.write().await;
    let err = engine
        .record_and_write_notification_until(
            "textDocument/didOpen",
            params.clone(),
            tokio::time::Instant::now(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Timeout recording diagnostics for notification 'textDocument/didOpen'"
    );
    drop(diagnostics);
    let sent = engine.sent.write().await;
    for (method, expected) in [
        (
            "textDocument/didChange",
            "Timeout recording sent text for notification 'textDocument/didChange'",
        ),
        (
            "textDocument/didClose",
            "Timeout recording closed text for notification 'textDocument/didClose'",
        ),
    ] {
        let err = engine
            .record_and_write_notification_until(
                method,
                params.clone(),
                tokio::time::Instant::now() + Duration::from_millis(5),
            )
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), expected);
    }
    drop(sent);
}
