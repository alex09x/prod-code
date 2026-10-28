/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `run_remote` and `run_polyglot_remote` against a scripted gateway on loopback: the
//! pre-flight sync is answered, then the script plays what a node sends after the command.

mod assertions;
mod writeback_lock;

use super::runner::{run_polyglot_remote, run_remote};
use super::types::PolyglotRemoteOutcome;
use crate::exec::runner_tests::assertions::assert_pulled;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ExecChanges, ExecChunk, ExecExit, FileDelta, ProdCodeCodec, RemoteExecCommand,
    RemoteExecFormat, RemoteExecLanguage, RemoteExecRequest, RemoteExecResult, RemoteExecStream,
    SyncProbeResponse, SyncResponse, WireMessage,
};
use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;

/// How the scripted node ends the stream after the script.
#[derive(Clone, Copy)]
enum End {
    Close,
    /// A frame header over the size limit: the client's decoder fails with an I/O error.
    Garbage,
}

/// A node that answers the pre-flight, plays `script` after the exec request and resolves to
/// that request.
async fn gateway(script: Vec<WireMessage>, end: End) -> (SocketAddr, JoinHandle<WireMessage>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        while let Some(Ok(msg)) = framed.next().await {
            match msg {
                WireMessage::SyncProbeRequest(req) => framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing: vec![],
                    }))
                    .await
                    .unwrap(),
                WireMessage::SyncRequest(req) => framed
                    .send(WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: req.client_workspace_root,
                        files_updated: 0,
                        files_deleted: 0,
                        bytes_transferred: 0,
                        duration_ms: 0,
                        workspace_was_fresh: false,
                        stale_paths: vec![],
                    }))
                    .await
                    .unwrap(),
                WireMessage::ExecRequest(_) | WireMessage::RemoteExecRequest(_) => {
                    for message in script {
                        framed.send(message).await.unwrap();
                    }
                    if matches!(end, End::Garbage) {
                        framed.get_mut().write_all(&[0xFF; 4]).await.unwrap();
                    }
                    return msg;
                }
                other => panic!("unexpected request {other:?}"),
            }
        }
        panic!("the client never sent the exec request");
    });
    (address, task)
}

fn exit(code: i32) -> WireMessage {
    WireMessage::ExecExit(ExecExit {
        exit_code: Some(code),
        duration_ms: 5,
        server_workspace_root: "/srv/ws".into(),
        timed_out: false,
        error: None,
        usage: None,
        platform: None,
    })
}

fn chunk(stderr: bool, text: Option<&str>) -> WireMessage {
    WireMessage::ExecChunk(ExecChunk {
        stderr,
        data: text.map(|t| t.as_bytes().to_vec()),
    })
}

fn delta(path: &str, content: Option<&str>) -> FileDelta {
    FileDelta {
        relative_path: path.into(),
        content: content.map(|c| c.as_bytes().to_vec()),
        is_executable: false,
    }
}

fn changes(files: Vec<FileDelta>) -> WireMessage {
    WireMessage::ExecChanges(ExecChanges { files })
}

async fn exec_error(script: Vec<WireMessage>, end: End) -> String {
    let root = tempfile::tempdir().unwrap();
    let (address, server) = gateway(script, end).await;
    let err = run_remote(
        address,
        root.path(),
        None,
        vec!["x".into()],
        vec![],
        20,
        true,
        |_, _| {},
    )
    .await
    .expect_err("the stream ends before an exit");
    server.await.unwrap();
    format!("{err:#}")
}

fn request() -> RemoteExecRequest {
    RemoteExecRequest {
        client_workspace_root: String::new(),
        base_workspace_name: None,
        language: RemoteExecLanguage::Rust,
        command: RemoteExecCommand::Check,
        args: vec![],
        env: vec![],
        format: RemoteExecFormat::Json,
        timeout_secs: 20,
        pull_changes: true,
        subdir: None,
        client_agent: None,
        client_host: None,
    }
}

async fn polyglot_error(script: Vec<WireMessage>, end: End) -> String {
    let root = tempfile::tempdir().unwrap();
    let (address, server) = gateway(script, end).await;
    let err = run_polyglot_remote(address, root.path(), request(), |_| {})
        .await
        .expect_err("the stream ends before a result");
    server.await.unwrap();
    format!("{err:#}")
}

fn make_future(path: &Path) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(3600))
        .unwrap();
}

/// Local files the scripted command "changed": one of every kind of delta.
fn seed_checkout(root: &Path) {
    std::fs::write(root.join("same.txt"), "same\n").unwrap();
    std::fs::write(root.join("fmt.rs"), "use b::B;\nuse a::A;\n").unwrap();
    std::fs::write(root.join("edit.rs"), "local edit\n").unwrap();
    std::fs::write(root.join("gone.txt"), "to delete\n").unwrap();
    make_future(&root.join("edit.rs"));
}

fn deltas() -> Vec<FileDelta> {
    vec![
        delta("same.txt", Some("same\n")),
        delta("fmt.rs", Some("use a::A;\nuse b::B;\n")),
        delta("edit.rs", Some("node edit\n")),
        delta("gone.txt", None),
        delta("new/created.txt", Some("created\n")),
        delta("../runner-escape-probe.txt", Some("outside\n")),
    ]
}

#[tokio::test]
async fn run_remote_streams_output_and_writes_back_changed_files() {
    let root = tempfile::tempdir().unwrap();
    seed_checkout(root.path());
    let script = vec![
        chunk(false, Some("out ")),
        chunk(true, Some("err")),
        chunk(false, None),
        WireMessage::Pong,
        WireMessage::LspPayload("{}".into()),
        changes(deltas()),
        exit(3),
    ];
    let (address, server) = gateway(script, End::Close).await;
    let mut seen: Vec<(bool, String)> = Vec::new();
    let outcome = run_remote(
        address,
        root.path(),
        Some("sub/dir"),
        vec!["cargo".into(), "fmt".into()],
        vec![("K".into(), "V".into())],
        20,
        true,
        |stderr, bytes| seen.push((stderr, String::from_utf8_lossy(bytes).into_owned())),
    )
    .await
    .unwrap();
    assert_eq!(outcome.exit.exit_code, Some(3));
    assert_eq!(outcome.exit.server_workspace_root, "/srv/ws");
    assert_eq!(
        seen,
        [(false, "out ".to_string()), (true, "err".to_string())]
    );
    assert_pulled(
        root.path(),
        &outcome.pulled_files,
        &outcome.relaid_files,
        &outcome.kept_files,
    );
    let mut code = outcome.changed_code();
    code.sort();
    assert_eq!(code, ["fmt.rs", "gone.txt", "new/created.txt"]);
    let WireMessage::ExecRequest(req) = server.await.unwrap() else {
        panic!("not an exec request");
    };
    assert_eq!(req.command, ["cargo", "fmt"]);
    assert_eq!(req.env, [("K".to_string(), "V".to_string())]);
    assert_eq!((req.timeout_secs, req.pull_changes), (20, true));
    assert_eq!(req.subdir.as_deref(), Some("sub/dir"));
    assert_eq!(req.client_workspace_root, root.path().to_string_lossy());
}

#[tokio::test]
async fn run_remote_refuses_an_empty_command() {
    let root = tempfile::tempdir().unwrap();
    let address: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let err = run_remote(
        address,
        root.path(),
        None,
        vec![],
        vec![],
        1,
        false,
        |_, _| {},
    )
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "empty command");
}

#[tokio::test]
async fn run_remote_names_what_is_unknown_when_the_stream_breaks() {
    let one_file = || changes(vec![delta("a.txt", Some("a\n"))]);
    let cases: Vec<(Vec<WireMessage>, End, &str)> = vec![
        (
            vec![],
            End::Close,
            "gateway closed the connection during exec; the command's result is unknown",
        ),
        (
            vec![one_file()],
            End::Close,
            "gateway closed the connection after sending 1 changed file(s); the command's exit code is unknown",
        ),
        (
            vec![],
            End::Garbage,
            "lost the connection to the gateway during exec (Frame size 4294967295 exceeds maximum 268435456); the command's result is unknown",
        ),
        (
            vec![one_file()],
            End::Garbage,
            "lost the connection to the gateway after receiving and applying 1 changed file(s) (Frame size 4294967295 exceeds maximum 268435456); the command's exit code is unknown",
        ),
        (
            vec![WireMessage::Disconnect {
                reason: "bye".into(),
            }],
            End::Close,
            "unexpected message during exec: Disconnect { reason: \"bye\" }",
        ),
    ];
    for (script, end, expected) in cases {
        assert_eq!(exec_error(script, end).await, expected);
    }
}

fn result(code: i32) -> RemoteExecResult {
    RemoteExecResult {
        exit_code: Some(code),
        duration_ms: 9,
        server_workspace_root: "/srv/ws".into(),
        timed_out: false,
        error: None,
        usage: None,
        platform: None,
        diagnostics: vec![],
        tests_passed: 2,
        tests_failed: 1,
        tests_skipped: 0,
        test_failures: vec![],
        benches: vec![],
    }
}

#[tokio::test]
async fn run_polyglot_remote_forwards_the_stream_and_writes_back_changed_files() {
    let root = tempfile::tempdir().unwrap();
    seed_checkout(root.path());
    let streamed = RemoteExecStream::Chunk(ExecChunk {
        stderr: true,
        data: Some(b"warn".to_vec()),
    });
    let script = vec![
        WireMessage::RemoteExecStream(streamed.clone()),
        WireMessage::Pong,
        WireMessage::LspPayload("{}".into()),
        changes(deltas()),
        WireMessage::RemoteExecResult(result(1)),
    ];
    let (address, server) = gateway(script, End::Close).await;
    let mut events = Vec::new();
    let outcome: PolyglotRemoteOutcome =
        run_polyglot_remote(address, root.path(), request(), |ev| events.push(ev))
            .await
            .unwrap();
    assert_eq!(events, [streamed]);
    assert_eq!(outcome.result, result(1));
    assert!(!outcome.result.ok());
    assert_pulled(
        root.path(),
        &outcome.pulled_files,
        &outcome.relaid_files,
        &outcome.kept_files,
    );
    let mut code = outcome.changed_code();
    code.sort();
    assert_eq!(code, ["fmt.rs", "gone.txt", "new/created.txt"]);
    let WireMessage::RemoteExecRequest(req) = server.await.unwrap() else {
        panic!("not a remote exec request");
    };
    assert_eq!(req, request());
}

#[tokio::test]
async fn run_polyglot_remote_names_what_is_unknown_when_the_stream_breaks() {
    let one_file = || changes(vec![delta("a.txt", Some("a\n"))]);
    let cases: Vec<(Vec<WireMessage>, End, &str)> = vec![
        (
            vec![],
            End::Close,
            "gateway closed the connection during remote exec; the command's result is unknown",
        ),
        (
            vec![one_file()],
            End::Close,
            "gateway closed the connection after sending 1 changed file(s); the command's exit code is unknown",
        ),
        (
            vec![],
            End::Garbage,
            "lost the connection to the gateway during remote exec (Frame size 4294967295 exceeds maximum 268435456); the command's result is unknown",
        ),
        (
            vec![one_file()],
            End::Garbage,
            "lost the connection to the gateway after receiving and applying 1 changed file(s) (Frame size 4294967295 exceeds maximum 268435456); the command's exit code is unknown",
        ),
        (
            vec![WireMessage::Disconnect {
                reason: "bye".into(),
            }],
            End::Close,
            "unexpected message during remote exec: Disconnect { reason: \"bye\" }",
        ),
    ];
    for (script, end, expected) in cases {
        assert_eq!(polyglot_error(script, end).await, expected);
    }
}
