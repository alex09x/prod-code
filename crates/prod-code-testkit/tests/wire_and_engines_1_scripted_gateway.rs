/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Every message kind the scripted gateway serves, driven over a real socket.

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeRequest, ProdCodeCodec, ReadFileRequest, ShadowHypothesis, ShadowRunRequest,
    SyncProbeRequest, SyncRequest, WireMessage,
};
use prod_code_testkit::answers;
use prod_code_testkit::{Answer, LSP_ERROR, ScriptedGateway};

/// The key of an answer that leaves its request unanswered (not re-exported by the crate).
const NO_RESPONSE: &str = "prod-code/no-response";
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

type Client = Framed<TcpStream, ProdCodeCodec>;

async fn connect(gw: &ScriptedGateway) -> Client {
    Framed::new(
        TcpStream::connect(gw.addr()).await.unwrap(),
        ProdCodeCodec::new(),
    )
}

async fn ask(c: &mut Client, msg: WireMessage) -> WireMessage {
    c.send(msg).await.unwrap();
    c.next().await.expect("an answer").expect("a frame")
}

fn handshake(versions: Option<Vec<u32>>, purpose: Option<&str>) -> WireMessage {
    WireMessage::HandshakeRequest(HandshakeRequest {
        protocol_version: prod_code_protocol::PROTOCOL_VERSION,
        supported_versions: versions,
        capabilities: None,
        client_name: "t".into(),
        client_pid: 1,
        auth_token: None,
        client_workspace_root: "/ws".into(),
        preferred_engine: None,
        base_workspace_name: None,
        engine_subpath: None,
        client_agent: None,
        client_host: None,
        purpose: purpose.map(str::to_string),
        redirect_count: 0,
    })
}

fn lsp(id: u64, method: &str, params: Value) -> WireMessage {
    WireMessage::LspPayload(
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string(),
    )
}

fn payload(msg: WireMessage) -> Value {
    match msg {
        WireMessage::LspPayload(s) => serde_json::from_str(&s).unwrap(),
        other => panic!("expected an LSP payload, got {other:?}"),
    }
}

#[tokio::test]
async fn sync_messages_are_answered_as_if_the_tree_were_held() {
    let gw = ScriptedGateway::start(|_, _| Value::Null).await;
    let mut c = connect(&gw).await;
    let probe = WireMessage::SyncProbeRequest(SyncProbeRequest {
        client_workspace_root: "/ws".into(),
        base_workspace_name: None,
        seed_from: None,
        files: Vec::new(),
    });
    match ask(&mut c, probe).await {
        WireMessage::SyncProbeResponse(r) => {
            assert_eq!(r.server_workspace_root, "/ws");
            assert!(!r.seeded && r.missing.is_empty());
            assert_eq!(r.files_deleted, 0);
        }
        other => panic!("{other:?}"),
    }
    let sync = WireMessage::SyncRequest(SyncRequest {
        client_workspace_root: "/other".into(),
        files: Vec::new(),
        clean_others: false,
        base_workspace_name: None,
    });
    match ask(&mut c, sync).await {
        WireMessage::SyncResponse(r) => {
            assert_eq!(r.server_workspace_root, "/other");
            assert_eq!(
                (r.files_updated, r.files_deleted, r.bytes_transferred),
                (0, 0, 0)
            );
            assert!(!r.workspace_was_fresh && r.stale_paths.is_empty());
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn handshake_shows_the_purpose_to_the_script_and_reads_its_hints() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let gw = ScriptedGateway::start(move |m, p| {
        if m == "prod-code/handshake" {
            log.lock().unwrap().push(p.clone());
            return json!({"engine_age_ms": 1234, "index_gated": true});
        }
        Value::Null
    })
    .await;
    let mut c = connect(&gw).await;
    match ask(
        &mut c,
        handshake(
            Some(vec![prod_code_protocol::PROTOCOL_VERSION]),
            Some("validation"),
        ),
    )
    .await
    {
        WireMessage::HandshakeResponse(r) => {
            assert_eq!(r.protocol_version, prod_code_protocol::PROTOCOL_VERSION);
            assert_eq!(r.server_workspace_root, "/ws");
            assert_eq!(r.detected_engine, "rust");
            assert_eq!(r.session_id, 1);
            assert_eq!(r.engine_age_ms, Some(1234));
            assert!(r.index_gated);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        *seen.lock().unwrap(),
        vec![json!({"purpose": "validation", "preferred_engine": null})]
    );

    // A script that says nothing leaves the hints unset.
    let quiet = ScriptedGateway::start(|_, _| json!({})).await;
    let mut c = connect(&quiet).await;
    match ask(&mut c, handshake(None, None)).await {
        WireMessage::HandshakeResponse(r) => {
            assert_eq!((r.engine_age_ms, r.index_gated), (None, false))
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_version_nobody_speaks_is_refused_with_a_disconnect() {
    let gw = ScriptedGateway::start(|_, _| Value::Null).await;
    let mut c = connect(&gw).await;
    match ask(&mut c, handshake(Some(vec![u32::MAX]), None)).await {
        WireMessage::Disconnect { reason } => {
            assert!(
                reason.starts_with("protocol negotiation failed: "),
                "{reason}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(
        c.next().await.is_none(),
        "the connection is closed after the refusal"
    );
    let mut c = connect(&gw).await;
    match ask(&mut c, handshake(Some(Vec::new()), None)).await {
        WireMessage::Disconnect { reason } => {
            assert!(reason.starts_with("protocol negotiation failed: "))
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn files_on_the_node_are_read_through_the_script() {
    let gw = ScriptedGateway::start(|m, p| {
        if m == "prod-code/readFile" && p["path"] == "/dep/lib.rs" {
            return json!("pub fn dep() {}\n");
        }
        Value::Null
    })
    .await;
    let mut c = connect(&gw).await;
    let read = |path: &str| {
        WireMessage::ReadFileRequest(ReadFileRequest {
            path: path.into(),
            max_bytes: 0,
        })
    };
    match ask(&mut c, read("/dep/lib.rs")).await {
        WireMessage::ReadFileResponse(r) => {
            assert_eq!(r.path, "/dep/lib.rs");
            assert_eq!(r.content.as_deref(), Some(b"pub fn dep() {}\n".as_slice()));
            assert_eq!(
                (r.error, r.truncated, r.is_executable),
                (None, false, Some(false))
            );
        }
        other => panic!("{other:?}"),
    }
    match ask(&mut c, read("/dep/missing.rs")).await {
        WireMessage::ReadFileResponse(r) => {
            assert_eq!(r.content, None);
            assert_eq!(r.error.as_deref(), Some("no such file: /dep/missing.rs"));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn lsp_requests_are_counted_notifications_are_shown_and_initialize_is_built_in() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let answer: Answer = Arc::new(move |m, p| {
        log.lock().unwrap().push((m.to_string(), p.clone()));
        json!({"echo": m})
    });
    let gw = ScriptedGateway::start_arc(answer).await;
    let mut c = connect(&gw).await;

    let init = payload(ask(&mut c, lsp(1, "initialize", json!({}))).await);
    assert_eq!(init["id"], 1);
    assert_eq!(
        init["result"],
        json!({"capabilities": {"hoverProvider": true}})
    );
    assert_eq!(gw.calls(), 0, "initialize is not counted");

    // Garbage and notifications get no answer; the next request still does.
    c.send(WireMessage::LspPayload("not json".into()))
        .await
        .unwrap();
    c.send(WireMessage::LspPayload(
        json!({"method": "textDocument/didOpen", "params": {"x": 1}}).to_string(),
    ))
    .await
    .unwrap();
    c.send(WireMessage::LspPayload(
        json!({"method": "initialized"}).to_string(),
    ))
    .await
    .unwrap();
    let hover = payload(ask(&mut c, lsp(2, "textDocument/hover", json!({"p": 1}))).await);
    assert_eq!(
        hover,
        json!({"jsonrpc": "2.0", "id": 2, "result": {"echo": "textDocument/hover"}})
    );
    assert_eq!(gw.calls(), 1);
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            ("textDocument/didOpen".to_string(), json!({"x": 1})),
            ("initialized".to_string(), Value::Null),
            ("textDocument/hover".to_string(), json!({"p": 1})),
        ]
    );
}

#[tokio::test]
async fn scripted_failures_go_back_as_json_rpc_errors() {
    let gw = ScriptedGateway::start(|m, _| match m {
        "a" => answers::failure("analyzer crashed"),
        "b" => answers::rpc_error(-32801, "content modified"),
        "c" => json!({ LSP_ERROR: {"code": 7, "message": "custom"} }),
        _ => json!(null),
    })
    .await;
    let mut c = connect(&gw).await;
    let a = payload(ask(&mut c, lsp(1, "a", Value::Null)).await);
    assert_eq!(
        a["error"],
        json!({"code": -32603, "message": "analyzer crashed"})
    );
    assert!(a.get("result").is_none());
    let b = payload(ask(&mut c, lsp(2, "b", Value::Null)).await);
    assert_eq!(
        b["error"],
        json!({"code": -32801, "message": "content modified"})
    );
    let cc = payload(ask(&mut c, lsp(3, "c", Value::Null)).await);
    assert_eq!(cc["error"], json!({"code": 7, "message": "custom"}));
    let d = payload(ask(&mut c, lsp(4, "d", Value::Null)).await);
    assert_eq!(d, json!({"jsonrpc": "2.0", "id": 4, "result": null}));
}

#[tokio::test]
async fn an_unanswered_request_leaves_the_connection_usable() {
    let gw = ScriptedGateway::start(|m, _| {
        if m == "slow" {
            json!({ NO_RESPONSE: true })
        } else {
            json!("ok")
        }
    })
    .await;
    let mut c = connect(&gw).await;
    c.send(lsp(1, "slow", Value::Null)).await.unwrap();
    let next = payload(ask(&mut c, lsp(2, "fast", Value::Null)).await);
    assert_eq!(
        next["id"], 2,
        "no answer was sent for the unanswered request"
    );
    assert_eq!(gw.calls(), 2);
}

#[tokio::test]
async fn a_scripted_redirect_is_sent_and_ends_the_connection() {
    let gw =
        ScriptedGateway::start(|_, _| json!({"redirect": "10.0.0.9:9400", "reason": "busy"})).await;
    let mut c = connect(&gw).await;
    match ask(&mut c, lsp(1, "textDocument/hover", Value::Null)).await {
        WireMessage::Redirect {
            target_addr,
            reason,
        } => {
            assert_eq!(target_addr, "10.0.0.9:9400");
            assert_eq!(reason.as_deref(), Some("busy"));
        }
        other => panic!("{other:?}"),
    }
    assert!(c.next().await.is_none());
    let gw = ScriptedGateway::start(|_, _| json!({"redirect": "h:1"})).await;
    let mut c = connect(&gw).await;
    match ask(&mut c, lsp(1, "x", Value::Null)).await {
        WireMessage::Redirect { reason, .. } => assert_eq!(reason, None),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn index_questions_carry_the_servers_own_request_and_the_busy_note() {
    let gw = ScriptedGateway::start(|m, p| match m {
        "prod-code/server-request" => {
            json!({"method": "window/workDoneProgress/create", "params": p})
        }
        "prod-code/busy" => json!({"title": "indexing", "for_ms": 5000}),
        "textDocument/references" => json!(["r"]),
        _ => json!("plain"),
    })
    .await;
    let mut c = connect(&gw).await;
    c.send(lsp(7, "textDocument/references", Value::Null))
        .await
        .unwrap();
    let request = payload(c.next().await.unwrap().unwrap());
    assert_eq!(request["method"], "window/workDoneProgress/create");
    assert_eq!(request["id"], 7, "it is given the question's id");
    assert_eq!(
        request["params"],
        json!({"method": "textDocument/references"})
    );
    let note = payload(c.next().await.unwrap().unwrap());
    assert_eq!(
        note["method"],
        prod_code_protocol::readiness::BUSY_NOTIFICATION
    );
    assert_eq!(note["params"], json!({"title": "indexing", "for_ms": 5000}));
    let answer = payload(c.next().await.unwrap().unwrap());
    assert_eq!(answer, json!({"jsonrpc": "2.0", "id": 7, "result": ["r"]}));
    // A question that is not answered from the index gets neither.
    let plain = payload(ask(&mut c, lsp(8, "textDocument/hover", Value::Null)).await);
    assert_eq!(plain["result"], "plain");
}

fn shadow(names: &[&str]) -> WireMessage {
    WireMessage::ShadowRunRequest(ShadowRunRequest {
        client_workspace_root: "/ws".into(),
        base_workspace_name: None,
        hypotheses: names
            .iter()
            .map(|n| ShadowHypothesis {
                name: n.to_string(),
                files: Vec::new(),
            })
            .collect(),
        command: vec!["cargo".into(), "test".into()],
        env: Vec::new(),
        timeout_secs: 0,
        subdir: None,
        parallel: 0,
        tail_bytes: 0,
        in_memory: false,
        client_agent: None,
        client_host: None,
    })
}

#[tokio::test]
async fn shadow_runs_follow_the_script() {
    let gw = ScriptedGateway::start(|m, p| match (m, p["command"][0].as_str()) {
        ("prod-code/shadow-run", Some("cargo")) if p["command"][1] == "test" => {
            json!({"exit_code": 3})
        }
        _ => Value::Null,
    })
    .await;
    let mut c = connect(&gw).await;
    match ask(&mut c, shadow(&["a", "b"])).await {
        WireMessage::ShadowRunResponse(r) => {
            assert_eq!(
                (r.server_workspace_root.as_str(), r.mode.as_str(), r.error),
                ("/ws", "mock", None)
            );
            let got: Vec<_> = r
                .results
                .iter()
                .map(|h| (h.name.as_str(), h.exit_code, h.timed_out))
                .collect();
            assert_eq!(got, [("a", Some(3), false), ("b", Some(3), false)]);
            assert_eq!(r.results[0].duration_ms, 10);
        }
        other => panic!("{other:?}"),
    }
    let silent = ScriptedGateway::start(|_, _| Value::Null).await;
    let mut c = connect(&silent).await;
    match ask(&mut c, shadow(&["a"])).await {
        WireMessage::ShadowRunResponse(r) => {
            assert!(r.results.is_empty());
            assert_eq!(
                r.error.as_deref(),
                Some("shadow run not supported by scripted gateway")
            );
        }
        other => panic!("{other:?}"),
    }
    let failing = ScriptedGateway::start(|_, _| json!({"error": "no overlay"})).await;
    let mut c = connect(&failing).await;
    match ask(&mut c, shadow(&["a"])).await {
        WireMessage::ShadowRunResponse(r) => assert_eq!(r.error.as_deref(), Some("no overlay")),
        other => panic!("{other:?}"),
    }
    let default_exit = ScriptedGateway::start(|_, _| json!({})).await;
    let mut c = connect(&default_exit).await;
    match ask(&mut c, shadow(&["z"])).await {
        WireMessage::ShadowRunResponse(r) => assert_eq!(r.results[0].exit_code, Some(0)),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn status_lists_the_engines_and_disconnect_ends_the_session() {
    let gw = ScriptedGateway::start(|_, _| Value::Null).await;
    let mut c = connect(&gw).await;
    match ask(&mut c, WireMessage::StatusRequest).await {
        WireMessage::StatusResponse(s) => {
            assert_eq!(s.server_pid, std::process::id());
            assert_eq!(
                s.detected_engines,
                [
                    "rust",
                    "go",
                    "typescript",
                    "javascript",
                    "python",
                    "cpp",
                    "c",
                    "swift",
                    "generic-lsp"
                ]
            );
            assert_eq!(
                (s.active_sessions, s.loaded_workspaces, s.cpu_count),
                (0, 0, Some(4))
            );
            assert_eq!(
                s.platform,
                Some(format!(
                    "{} {}",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ))
            );
        }
        other => panic!("{other:?}"),
    }
    c.send(WireMessage::Disconnect {
        reason: "bye".into(),
    })
    .await
    .unwrap();
    assert!(c.next().await.is_none());
}
