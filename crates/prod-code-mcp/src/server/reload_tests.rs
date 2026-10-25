/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{reload::Reload, transport::serve_with_reload};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::codec::Framed;

async fn read_json(reader: &mut BufReader<tokio::io::DuplexStream>) -> Value {
    let mut line = String::new();
    assert_ne!(
        reader.read_line(&mut line).await.unwrap(),
        0,
        "transport closed"
    );
    serde_json::from_str(&line).unwrap()
}

async fn exercise_reload(finish: bool, partial_input: bool) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (read, write) = tokio::io::split(server);
    let root = tempfile::tempdir().unwrap();
    let mut reload = Reload::new(None);
    reload.cancel_safe_input = true; // DuplexStream has no uncancellable read worker.
    reload.exe = Some("unused-test-executable".into());
    reload.timeout = Duration::from_millis(50);
    let requested = reload.requested.clone();
    let notify = reload.notify.clone();
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = attempts.clone();
    reload.reexec = Box::new(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        std::io::Error::other("simulated exec failure")
    });
    let handle = tokio::spawn(serve_with_reload(
        listener.local_addr().unwrap(),
        root.path().into(),
        false,
        BufReader::new(read),
        write,
        reload,
    ));
    let mut client = BufReader::new(client);
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        client.get_mut().write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":\"active\",\"method\":\"tools/call\",\"params\":{\"name\":\"code_status\"}}\n"
        ).await.unwrap();
        let (socket, _) = listener.accept().await.unwrap();
        let mut gateway = Framed::new(socket, ProdCodeCodec::new());
        assert!(matches!(gateway.next().await.unwrap().unwrap(), WireMessage::StatusRequest));
        if partial_input {
            client.get_mut().write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"ping\"}\n{\"jsonrpc\":").await.unwrap();
            assert_eq!(read_json(&mut client).await["id"], 0);
        }
        requested.store(true, Ordering::Release);
        notify.notify_one();
        if finish {
            // A normal handler result, including a gateway error, must be flushed before exec.
            gateway.send(WireMessage::Disconnect { reason: "test result".into() }).await.unwrap();
        }
        let response = read_json(&mut client).await;
        assert_eq!(response["id"], "active");
        if finish {
            assert_eq!(response["result"]["isError"], true);
        } else {
            assert_eq!(response["error"]["code"], -32001);
            assert_eq!(response["error"]["data"]["retryable"], true);
            assert!(response["error"]["message"].as_str().unwrap().contains("retry"));
        }
        if partial_input {
            assert_eq!(attempts.load(Ordering::SeqCst), 0, "partial input lost at exec");
            client.get_mut().write_all(b"\"2.0\",\"id\":1,\"method\":\"ping\"}\n").await.unwrap();
            assert_eq!(read_json(&mut client).await["id"], 1);
        }
        let notification = read_json(&mut client).await;
        assert_eq!(notification["method"], "notifications/tools/list_changed");
        client.get_mut().write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n").await.unwrap();
        assert_eq!(read_json(&mut client).await, json!({"jsonrpc":"2.0","id":2,"result":{}}));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }).await;
    drop(client);
    handle.abort();
    let _ = handle.await;
    assert!(
        result.is_ok(),
        "reload left an in-flight call unanswered beyond its drain deadline"
    );
}

#[tokio::test]
async fn server_reload_deadline_returns_retryable_error_and_keeps_transport() {
    exercise_reload(false, false).await;
}

#[tokio::test]
async fn server_reload_delivers_completed_call_before_notification() {
    exercise_reload(true, false).await;
}

#[tokio::test]
async fn server_reload_drains_active_calls_without_losing_partial_input() {
    exercise_reload(false, true).await;
}

#[tokio::test]
async fn server_reload_deadline_resolves_all_ids_once_and_skips_cancelled_calls() {
    use super::reload::{ActiveCalls, ToolCalls, drain_for_reload};
    let mut calls = ToolCalls::new();
    let mut active = ActiveCalls::new();
    for id in [json!(7), json!("string-id"), Value::Null] {
        let handle = calls.spawn(std::future::pending());
        active.insert(serde_json::to_string(&id).unwrap(), handle);
    }
    // A notification has no id, and a cancelled request has already left the active map.
    calls.spawn(std::future::pending());
    let cancelled = calls.spawn(std::future::pending());
    cancelled.abort();
    let handle = calls.spawn(async {
        (
            Some("8".into()),
            None,
            Some("{\"jsonrpc\":\"2.0\",\"id\":8,\"result\":{}}\n".into()),
        )
    });
    active.insert("8".into(), handle);
    let mut output = Vec::new();
    let mut remote = "127.0.0.1:1".parse().unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        drain_for_reload(
            &mut calls,
            &mut active,
            &mut remote,
            &mut output,
            Duration::from_millis(50),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(calls.is_empty());
    assert!(active.is_empty());
    let responses: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 4);
    for id in [json!(7), json!("string-id"), Value::Null, json!(8)] {
        let matching: Vec<_> = responses
            .iter()
            .filter(|response| response["id"] == id)
            .collect();
        assert_eq!(matching.len(), 1, "{id}: {responses:?}");
        if id == 8 {
            assert_eq!(matching[0]["result"], json!({}));
        } else {
            assert_eq!(matching[0]["error"]["data"]["retryable"], true);
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn server_reload_reexec_preserves_standard_streams() {
    use std::os::unix::fs::PermissionsExt;
    const CHILD_EXE: &str = "PROD_CODE_TEST_REEXEC_TARGET";
    if let Some(exe) = std::env::var_os(CHILD_EXE) {
        panic!(
            "exec failed: {}",
            crate::hot_reload::reexec(std::path::Path::new(&exe))
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("replacement");
    std::fs::write(&script,
        "#!/bin/sh\nprintf 'reload:%s\\n' \"$PROD_CODE_MCP_RESUMED\"\nIFS= read -r request\nprintf 'received:%s\\n' \"$request\"\n"
    ).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "server::reload_tests::server_reload_reexec_preserves_standard_streams",
            "--nocapture",
        ])
        .env(CHILD_EXE, &script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"ping\n")
        .await
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("reload:1\n"), "{stdout}");
    assert!(stdout.contains("received:ping\n"), "{stdout}");
}
