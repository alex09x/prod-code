/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    ExecutableFingerprint, LspResumeState, RESUMED_STATE_ENV, WATCH_INTERVAL_ENV, WATCH_PATH_ENV,
    load_resume_state, restore_resumed_session, spawn_executable_watcher, write_resume_state,
};
use crate::lsp_bridge::encoding::POSITION_ENCODING_UTF16;
use crate::lsp_bridge::state::LspStateTracker;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{AnyStream, ProdCodeCodec, WireMessage};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_util::codec::Framed;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
fn write_executable(path: &std::path::Path, contents: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o755)
        .open(path)
        .expect("open executable");
    file.write_all(contents).expect("write executable");
}

#[test]
#[cfg(unix)]
fn executable_fingerprint_detects_atomic_replacement_and_rejects_non_executable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin_path = dir.path().join("prod-code");
    write_executable(&bin_path, b"#!/bin/sh\necho v1\n");

    let before = ExecutableFingerprint::capture(&bin_path).expect("fingerprint v1");

    let staging = dir.path().join("prod-code.new");
    write_executable(&staging, b"#!/bin/sh\necho v2-updated\n");
    std::fs::rename(&staging, &bin_path).expect("atomic rename");

    let after = ExecutableFingerprint::capture(&bin_path).expect("fingerprint v2");
    assert_ne!(
        before, after,
        "atomic binary replacement must change fingerprint"
    );

    let non_exec = dir.path().join("plain.txt");
    std::fs::write(&non_exec, b"not executable").expect("write plain file");
    assert!(
        ExecutableFingerprint::capture(&non_exec).is_none(),
        "non-executable file must not be treated as a valid binary"
    );
}

#[test]
fn write_and_load_resume_state_round_trips_tracker_and_removes_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut tracker = LspStateTracker::default();
    tracker.record_client_message(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
        POSITION_ENCODING_UTF16,
    );
    tracker.record_client_message(
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
        POSITION_ENCODING_UTF16,
    );
    tracker.record_client_message(
        r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///workspace/src/lib.rs","languageId":"rust","version":1,"text":"pub fn v1() {}\n"}}}"#,
        POSITION_ENCODING_UTF16,
    );
    tracker.record_client_message(
        r#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///workspace/src/lib.rs","version":2},"contentChanges":[{"text":"pub fn v2() {}\n"}]}}"#,
        POSITION_ENCODING_UTF16,
    );

    let state = LspResumeState {
        remote: "127.0.0.1:9400".parse().expect("socket addr"),
        position_encoding: POSITION_ENCODING_UTF16,
        tracker,
        deferred_editor_frames: vec![
            r#"{"jsonrpc":"2.0","id":9,"method":"textDocument/hover","params":{}}"#.to_string(),
        ],
        pending_requests: vec![(
            serde_json::json!(7),
            "textDocument/definition".to_string(),
            r#"{"jsonrpc":"2.0","id":7,"method":"textDocument/definition","params":{}}"#
                .to_string(),
        )],
    };

    let path = write_resume_state(&state).expect("write resume state");
    assert!(path.is_file(), "resume state file should exist before load");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = std::fs::metadata(&path).expect("metadata").mode() & 0o777;
        assert_eq!(mode, 0o600, "resume state file must have 0600 permissions");
    }

    unsafe {
        std::env::set_var(RESUMED_STATE_ENV, &path);
    }
    let loaded = load_resume_state().expect("load resume state");
    assert!(
        !path.exists(),
        "resume state file must be removed after load"
    );
    assert!(
        std::env::var_os(RESUMED_STATE_ENV).is_none(),
        "resume env var must be cleared after load"
    );
    assert_eq!(loaded.remote, state.remote);
    assert_eq!(loaded.position_encoding, POSITION_ENCODING_UTF16);
    assert!(loaded.tracker.initialized_sent);
    let doc = loaded
        .tracker
        .open_documents
        .get("file:///workspace/src/lib.rs")
        .expect("tracked document");
    assert_eq!(doc.version, 2);
    assert_eq!(doc.text, "pub fn v2() {}\n");
    assert_eq!(loaded.deferred_editor_frames.len(), 1);
    assert_eq!(loaded.pending_requests.len(), 1);
}

#[tokio::test]
#[cfg(unix)]
async fn watcher_detects_binary_replacement_and_notifies_bridge() {
    let (watched, rx, handle, _dir) = {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let watched = dir.path().join("prod-code");
        write_executable(&watched, b"#!/bin/sh\necho old\n");

        unsafe {
            std::env::set_var(WATCH_PATH_ENV, &watched);
            std::env::set_var(WATCH_INTERVAL_ENV, "25");
        }
        let (handle, rx) = spawn_executable_watcher();
        unsafe {
            std::env::remove_var(WATCH_PATH_ENV);
            std::env::remove_var(WATCH_INTERVAL_ENV);
        }
        (watched, rx, handle, dir)
    };

    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    let replacement = watched.with_extension("next");
    write_executable(&replacement, b"#!/bin/sh\necho new-binary-version\n");
    std::fs::rename(&replacement, &watched).expect("replace binary");

    let notified = tokio::time::timeout(std::time::Duration::from_secs(2), rx)
        .await
        .expect("watcher should fire within timeout")
        .expect("watcher channel should yield path");
    assert_eq!(notified, watched);
    handle.abort();
}

#[tokio::test]
async fn restore_resumed_session_replays_initialize_and_open_documents() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let workspace = tempfile::tempdir().expect("workspace");
    let src_dir = workspace.path().join("src");
    std::fs::create_dir_all(&src_dir).expect("create src");
    let file_path = src_dir.join("lib.rs");
    std::fs::write(&file_path, "pub fn live() {}\n").expect("write lib.rs");
    let file_uri = url::Url::from_file_path(&file_path)
        .expect("file uri")
        .to_string();

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        let mut received_methods = Vec::new();
        let mut opened_text = String::new();
        while let Some(Ok(WireMessage::LspPayload(raw))) = framed.next().await {
            let val: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
            let method = val
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or_default()
                .to_string();
            received_methods.push(method.clone());
            if method == "initialize" {
                let id = val.get("id").cloned().unwrap_or(serde_json::json!(1));
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "capabilities": {
                            "positionEncoding": "utf-16"
                        }
                    }
                });
                framed
                    .send(WireMessage::LspPayload(resp.to_string()))
                    .await
                    .expect("send initialize result");
            } else if method == "textDocument/didOpen" {
                opened_text = val
                    .pointer("/params/textDocument/text")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string();
                break;
            }
        }
        (received_methods, opened_text)
    });

    let client = TcpStream::connect(addr).await.expect("connect");
    let framed = Framed::new(AnyStream::Tcp(client), ProdCodeCodec::new());
    let (mut socket_tx, mut socket_rx) = framed.split();

    let mut tracker = LspStateTracker::default();
    tracker.record_client_message(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
        POSITION_ENCODING_UTF16,
    );
    tracker.record_client_message(
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
        POSITION_ENCODING_UTF16,
    );
    tracker.record_client_message(
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": file_uri,
                    "languageId": "rust",
                    "version": 3,
                    "text": "pub fn hot_reloaded() {}\n"
                }
            }
        })
        .to_string(),
        POSITION_ENCODING_UTF16,
    );

    let files = prod_code_client::editor_files::RemoteFiles::new(
        addr,
        workspace.path(),
        workspace.path(),
        &prod_code_client::editor_files::default_cache(),
    );
    let position_encoding = AtomicU8::new(POSITION_ENCODING_UTF16);
    let (_tx, rx) = tokio::sync::mpsc::channel(4);
    let editor_frames = Arc::new(Mutex::new(rx));
    let mut deferred = VecDeque::new();
    let pending = Arc::new(Mutex::new(Vec::new()));
    let editor_out = Mutex::new(tokio::io::stdout());
    let trace = None;

    let replay_after_init = restore_resumed_session(
        &mut socket_tx,
        &mut socket_rx,
        &tracker,
        &files,
        &position_encoding,
        &editor_frames,
        &mut deferred,
        &pending,
        &editor_out,
        &trace,
    )
    .await
    .expect("restore resumed session");
    assert!(!replay_after_init);

    let (methods, opened_text) = server_task.await.expect("server task");
    assert_eq!(
        methods,
        vec!["initialize", "initialized", "textDocument/didOpen"]
    );
    assert_eq!(opened_text, "pub fn hot_reloaded() {}\n");
}
