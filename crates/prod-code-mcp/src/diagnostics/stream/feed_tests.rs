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
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

#[tokio::test]
async fn runtime_paths_2_stream_restore_corrupt_and_tombstone_reset() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let file = Path::new("a.json");
    let remote = "127.0.0.1:1".parse().unwrap();
    let manager = StreamSessionManager::new();
    let key = StreamSessionKey::new(remote, root, file, "restore");
    let disk = stream_session_disk_path(&key);
    for data in ["not json", "{}", "{\"accumulated\":1,\"chunk_count\":2}"] {
        write_private_session_file(&disk, data).unwrap();
        let err = manager.feed_chunk(remote, root, file, "restore", "", false, false, false).await.unwrap_err();
        assert_eq!(err.to_string(), "stream session 'restore' not found or was evicted; pass reset: true to start a new stream");
    }
    write_private_session_file(&disk, "{\"accumulated\":\"[\",\"chunk_count\":2}").unwrap();
    let result = manager.feed_chunk(remote, root, file, "restore", "1", false, false, false).await.unwrap();
    assert_eq!((result.chunk_index, result.accumulated_bytes, result.checkpoint_evaluated), (3, 2, false));
    assert_eq!(result.summary, "buffered chunk 3 (2 bytes)");
    let result = manager.feed_chunk(remote, root, file, "restore", "]", true, false, false).await.unwrap();
    assert_eq!((result.chunk_index, result.accumulated_bytes, result.intercepted), (4, 3, false));
    assert_eq!(result.summary, "stream generation validated clean: 0 hallucinations intercepted");
    assert_eq!(result.final_report.unwrap().errors, 0);
    assert!(!disk.exists());
    let err = manager.feed_chunk(remote, root, file, "restore", "", false, false, false).await.unwrap_err();
    assert_eq!(err.to_string(), "stream session 'restore' has expired or was terminated; pass reset: true to start a new stream");
    let result = manager.feed_chunk(remote, root, file, "restore", "[]", true, true, false).await.unwrap();
    assert_eq!(result.chunk_index, 1);
    assert_eq!(result.final_report.unwrap().items.len(), 0);
    manager.reset_session(remote, root, file, "restore");
}

#[tokio::test]
async fn runtime_paths_2_stream_size_and_capacity_refusals() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let file = Path::new("a.json");
    let remote = "127.0.0.1:1".parse().unwrap();
    let manager = StreamSessionManager::new();
    let error = manager.feed_chunk(remote, root, file, "large", &"x".repeat(MAX_STREAM_CHUNK_BYTES + 1), false, true, false).await.unwrap_err();
    assert_eq!(error.to_string(), format!("stream chunk size ({} bytes) exceeds maximum limit of {} bytes", MAX_STREAM_CHUNK_BYTES + 1, MAX_STREAM_CHUNK_BYTES));
    for i in 0..MAX_ACTIVE_STREAM_SESSIONS {
        let key = StreamSessionKey::new(remote, root, file, &i.to_string());
        manager.sessions.lock().unwrap().insert(key.clone(), StreamSession { key, accumulated: String::new(), chunk_count: 0, last_activity: Instant::now() });
    }
    let key = StreamSessionKey::new(remote, root, file, "capacity");
    let disk = stream_session_disk_path(&key);
    write_private_session_file(&disk, "{\"accumulated\":\"\",\"chunk_count\":1}").unwrap();
    for reset in [false, true] {
        let err = manager.feed_chunk(remote, root, file, "capacity", "", false, reset, false).await.unwrap_err();
        assert_eq!(err.to_string(), format!("maximum concurrent active stream sessions limit ({MAX_ACTIVE_STREAM_SESSIONS}) reached; retry later or reset unused sessions"));
    }
    manager.sessions.lock().unwrap().clear();
    manager.sessions.lock().unwrap().insert(key.clone(), StreamSession { key: key.clone(), accumulated: "x".repeat(MAX_STREAM_SESSION_BYTES), chunk_count: 1, last_activity: Instant::now() });
    let err = manager.feed_chunk(remote, root, file, "capacity", "x", false, false, false).await.unwrap_err();
    assert_eq!(err.to_string(), format!("stream session accumulated buffer ({} bytes) exceeds maximum limit of {} bytes", MAX_STREAM_SESSION_BYTES + 1, MAX_STREAM_SESSION_BYTES));
    assert!(!manager.sessions.lock().unwrap().contains_key(&key));
    assert_eq!(manager.tombstone_count(), 1);
    assert!(!disk.exists());
    manager.reset_session(remote, root, file, "capacity");
}

#[tokio::test]
async fn runtime_paths_2_stream_checkpoints_errors_and_final_interception() {
    let ws = Workspace::new(&[("a.rs", "fn a() {}")]);
    let gateway = ScriptedGateway::start(|method, _| {
        if method == "textDocument/diagnostic" { answers::error_at(2, 3, "E0425", "cannot find value `missing` in this scope") } else { serde_json::json!([]) }
    }).await;
    let manager = StreamSessionManager::new();
    let file = Path::new("new.rs");
    let first = manager.feed_chunk(gateway.addr(), &ws.root(), file, "semantic", "missing;", false, true, false).await.unwrap();
    assert_eq!(first.summary, "checkpoint at chunk 1 found 1 provisional error(s); semantic interceptions are deferred until close");
    assert!(!first.intercepted);
    let last = manager.feed_chunk(gateway.addr(), &ws.root(), file, "semantic", "", true, false, false).await.unwrap();
    assert_eq!(last.summary, "intercepted UnresolvedIdentifier at chunk 2 (line 2:3)");
    assert_eq!(last.interception.unwrap().message, "cannot find value `missing` in this scope");
    assert_eq!(last.final_report.unwrap().errors, 1);
    assert!(last.intercepted);
    manager.reset_session(gateway.addr(), &ws.root(), file, "semantic");
    let unsupported = Path::new("a.png");
    let partial = manager.feed_chunk(gateway.addr(), &ws.root(), unsupported, "unsupported", "x;", false, true, false).await.unwrap();
    let final_error = manager.feed_chunk(gateway.addr(), &ws.root(), unsupported, "unsupported", "", true, false, false).await.unwrap_err().to_string();
    assert_eq!(partial.summary, format!("checkpoint at chunk 1 skipped due to intermediate syntax: {final_error}"));
    assert!(!partial.intercepted);
    manager.reset_session(gateway.addr(), &ws.root(), unsupported, "unsupported");
}
