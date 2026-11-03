/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::reload::Reload;
use super::transport::serve_with_reload;
use super::transport_serve_tests::{next_response, send};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::io::BufReader;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_1814_reload_drain_refuses_new_work_before_its_budget_expires() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname=\"reload_budget\"\nversion=\"0.1.0\"\n",
        ),
        ("src/lib.rs", "pub fn target() {}\n"),
    ]);
    let queried = Arc::new(tokio::sync::Notify::new());
    let seen = queried.clone();
    let node = ScriptedGateway::start(move |method, _| {
        if method.starts_with("textDocument/") {
            seen.notify_one();
            json!({"prod-code/no-response": true})
        } else {
            serde_json::Value::Null
        }
    })
    .await;
    let mut reload = Reload::new(None);
    reload.exe = Some("unused-test-executable".into());
    reload.timeout = Duration::from_secs(2);
    reload.cancel_safe_input = true;
    let requested = reload.requested.clone();
    let notify = reload.notify.clone();
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    reload.reexec = Box::new(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        std::io::Error::other("simulated exec failure")
    });
    let (client, server) = tokio::io::duplex(65536);
    let (read, write) = tokio::io::split(server);
    let handle = tokio::spawn(serve_with_reload(
        node.addr(),
        ws.root(),
        false,
        BufReader::new(read),
        write,
        reload,
    ));
    let mut client = BufReader::new(client);
    send(
        &mut client,
        &json!({"jsonrpc":"2.0", "id":1, "method":"tools/call",
        "params":{"name":"code_definition", "arguments":{
            "path":"src/lib.rs", "line":1, "character":8, "budget_ms":5000}}})
        .to_string(),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), queried.notified())
        .await
        .unwrap();
    requested.store(true, Ordering::Release);
    notify.notify_one();
    // The old in-flight call remains pending. Neither new work nor the old catalog may be
    // served after reload starts, and reading these requests must not wait for the drain.
    send(
        &mut client,
        &json!({"jsonrpc":"2.0", "id":2, "method":"tools/call",
        "params":{"name":"code_definition", "arguments":{
            "path":"src/lib.rs", "line":1, "character":8, "budget_ms":120}}})
        .to_string(),
    )
    .await;
    send(
        &mut client,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
    )
    .await;
    let mut ids = Vec::new();
    for _ in 0..2 {
        let response = tokio::time::timeout(Duration::from_millis(600), next_response(&mut client))
            .await
            .expect("new requests must not wait two seconds for old work");
        assert_eq!(response["error"]["code"], -32001, "{response}");
        assert_eq!(response["error"]["data"]["started"], false, "{response}");
        assert_eq!(response["error"]["data"]["retryable"], true, "{response}");
        ids.push(response["id"].as_u64().unwrap());
    }
    ids.sort();
    assert_eq!(ids, [2, 3]);
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    send(
        &mut client,
        r#"{"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":1}}"#,
    )
    .await;
    let changed = tokio::time::timeout(Duration::from_secs(3), next_response(&mut client))
        .await
        .unwrap();
    assert_eq!(changed["method"], "notifications/tools/list_changed");
    send(&mut client, r#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#).await;
    let ping = tokio::time::timeout(Duration::from_secs(1), next_response(&mut client))
        .await
        .unwrap();
    assert_eq!(ping["id"], 4);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    drop(client);
    tokio::time::timeout(Duration::from_secs(3), handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
