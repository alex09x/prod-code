/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod lsp_reconnect_support;

use lsp_reconnect_support::{interruptible_proxy, response, spawn_bridge};
use prod_code_client::editor_files::write_frame;
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::BufReader;

#[tokio::test]
async fn default_bridge_recovers_after_gateway_restart_and_replays_unsaved_buffer() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn saved() {}\n")]);
    let cache = tempfile::tempdir().unwrap();
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&opened);
    let changed = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_changes = Arc::clone(&changed);
    let old_hover_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted_old_hover = Arc::clone(&old_hover_calls);
    let cancelled_hover_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted_cancelled_hover = Arc::clone(&cancelled_hover_calls);
    let gateway = ScriptedGateway::start(move |method, params| {
        if method == "textDocument/didOpen" {
            if let Some(text) = params["textDocument"]["text"].as_str() {
                captured.lock().unwrap().push(text.to_owned());
            }
        }
        if method == "textDocument/didChange" {
            if let Some(text) = params["contentChanges"][0]["text"].as_str() {
                captured_changes.lock().unwrap().push(text.to_owned());
            }
        }
        if method == "textDocument/hover" {
            if params["position"]["character"] == 1 {
                if counted_old_hover.fetch_add(1, std::sync::atomic::Ordering::AcqRel) == 0 {
                    json!({"prod-code/no-response": true})
                } else {
                    json!({"contents": "stale"})
                }
            } else if params["position"]["character"] == 2 {
                counted_cancelled_hover.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                json!({"prod-code/no-response": true})
            } else {
                json!({"contents": "recovered"})
            }
        } else {
            Value::Null
        }
    })
    .await;
    let (addr, down, _) = interruptible_proxy(gateway.addr()).await;
    let trace_path = cache.path().join("lsp-trace.log");
    let mut child = spawn_bridge(&ws, cache.path(), addr, Some(&trace_path));
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}})
            .to_string(),
    )
    .await
    .unwrap();
    assert_eq!(response(&mut stdout, 1).await["id"], 1);
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}).to_string(),
    )
    .await
    .unwrap();
    let uri = format!("file://{}/src/lib.rs", ws.root().display());
    let unsaved = "pub fn unsaved_edit() {}\n";
    write_frame(&mut stdin, &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"rust","version":1,"text":unsaved}}}).to_string()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while opened.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("initial unsaved buffer not observed");
    write_frame(&mut stdin, &json!({"jsonrpc":"2.0","id":7,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":{"line":0,"character":1}}}).to_string()).await.unwrap();
    write_frame(&mut stdin, &json!({"jsonrpc":"2.0","id":8,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":{"line":0,"character":2}}}).to_string()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while old_hover_calls.load(std::sync::atomic::Ordering::Acquire) == 0
            || cancelled_hover_calls.load(std::sync::atomic::Ordering::Acquire) == 0
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("initial hover was not observed");
    down.send(1).unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":8}}).to_string(),
    )
    .await
    .unwrap();
    let edited_offline = "pub fn edited_while_offline_161() {}\n";
    for version in 2..=161 {
        let text = format!("pub fn edited_while_offline_{version}() {{}}\n");
        write_frame(&mut stdin, &json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":text}]}}).to_string()).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(4750)).await;
    down.send(0).unwrap();
    assert!(
        child.try_wait().unwrap().is_none(),
        "LSP process exited during gateway outage"
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if opened
                .lock()
                .unwrap()
                .iter()
                .filter(|text| text.as_str() == unsaved)
                .count()
                >= 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("unsaved buffer not replayed after gateway returned");
    tokio::time::timeout(Duration::from_secs(10), async {
        while changed
            .lock()
            .unwrap()
            .last()
            .is_none_or(|text| text != edited_offline)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("edit made during outage not forwarded after reconnect");
    assert_eq!(
        changed.lock().unwrap().len(),
        160,
        "offline edits must remain ordered"
    );
    assert_eq!(
        response(&mut stdout, 7).await["error"]["code"],
        -32801,
        "stale in-flight response must not be replayed"
    );
    assert_eq!(
        old_hover_calls.load(std::sync::atomic::Ordering::Acquire),
        1
    );
    assert_eq!(
        cancelled_hover_calls.load(std::sync::atomic::Ordering::Acquire),
        1,
        "cancelled request must not be replayed"
    );
    write_frame(&mut stdin, &json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":{"line":0,"character":3}}}).to_string()).await.unwrap();
    assert_eq!(
        response(&mut stdout, 2).await["result"]["contents"],
        "recovered"
    );
    let trace = std::fs::read_to_string(trace_path).unwrap();
    assert!(trace.contains(&format!("client_version={}", env!("CARGO_PKG_VERSION"))));
    assert!(trace.contains("reconnect attempt attempt="), "{trace}");
    assert!(trace.contains("reconnect restored attempt="), "{trace}");
    assert!(!trace.contains(unsaved));
    assert!(!trace.contains(edited_offline));
}

#[tokio::test]
async fn editor_can_shutdown_cleanly_while_gateway_is_unavailable() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn sample() {}\n")]);
    let cache = tempfile::tempdir().unwrap();
    let initialized = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = Arc::clone(&initialized);
    let gateway = ScriptedGateway::start(move |method, _| {
        if method == "initialized" {
            seen.store(true, std::sync::atomic::Ordering::Release);
        }
        Value::Null
    })
    .await;
    let (addr, down, blackholes) = interruptible_proxy(gateway.addr()).await;
    let mut child = spawn_bridge(&ws, cache.path(), addr, None);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}})
            .to_string(),
    )
    .await
    .unwrap();
    assert_eq!(response(&mut stdout, 1).await["id"], 1);
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}).to_string(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !initialized.load(std::sync::atomic::Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("initialization not forwarded");
    // Leave new connections open without answering the sync/handshake, as a stalled peer can.
    down.send(2).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while blackholes.load(std::sync::atomic::Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("reconnect did not reach the stalled peer");
    assert!(
        child.try_wait().unwrap().is_none(),
        "bridge exited before editor shutdown"
    );
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}).to_string(),
    )
    .await
    .unwrap();
    assert_eq!(response(&mut stdout, 2).await["result"], Value::Null);
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"exit","params":null}).to_string(),
    )
    .await
    .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.code(), Some(0));
}

#[tokio::test]
async fn pending_semantic_token_delta_is_replaced_by_full_request_after_reconnect() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn sample() {}\n")]);
    let cache = tempfile::tempdir().unwrap();
    let delta_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = Arc::clone(&delta_calls);
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "textDocument/semanticTokens/full/delta" => {
            if captured.fetch_add(1, std::sync::atomic::Ordering::AcqRel) == 0 {
                json!({"prod-code/no-response":true})
            } else {
                json!({"edits":[]})
            }
        }
        "textDocument/semanticTokens/full" => json!({"data":[0,0,1,1,0]}),
        _ => Value::Null,
    })
    .await;
    let (addr, down, _) = interruptible_proxy(gateway.addr()).await;
    let mut child = spawn_bridge(&ws, cache.path(), addr, None);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}})
            .to_string(),
    )
    .await
    .unwrap();
    assert_eq!(response(&mut stdout, 1).await["id"], 1);
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"initialized","params":{}}).to_string(),
    )
    .await
    .unwrap();
    let uri = format!("file://{}/src/lib.rs", ws.root().display());
    write_frame(&mut stdin, &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"rust","version":1,"text":"pub fn sample() {}\n"}}}).to_string()).await.unwrap();
    write_frame(&mut stdin, &json!({"jsonrpc":"2.0","id":9,"method":"textDocument/semanticTokens/full/delta","params":{"textDocument":{"uri":uri},"previousResultId":"old-session-token"}}).to_string()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while delta_calls.load(std::sync::atomic::Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("initial delta request not observed");
    down.send(1).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    down.send(0).unwrap();
    assert_eq!(
        response(&mut stdout, 9).await["result"]["data"],
        json!([0, 0, 1, 1, 0])
    );
    assert_eq!(
        delta_calls.load(std::sync::atomic::Ordering::Acquire),
        1,
        "old session token must not be sent to the new gateway"
    );
}
