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
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::io::BufReader;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_shutdown_is_answered_when_gateway_disconnects() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn sample() {}\n")]);
    let cache = tempfile::tempdir().unwrap();
    let seen = Arc::new(AtomicBool::new(false));
    let received = Arc::clone(&seen);
    let gateway = ScriptedGateway::start(move |method, _| {
        if method == "shutdown" {
            received.store(true, Ordering::Release);
        }
        if matches!(method, "shutdown" | "textDocument/hover") {
            json!({"prod-code/no-response":true})
        } else {
            Value::Null
        }
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
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":99,"method":"textDocument/hover","params":{}}).to_string(),
    )
    .await
    .unwrap();
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}).to_string(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !seen.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("gateway did not receive shutdown");
    down.send(2).unwrap();
    let interrupted = tokio::time::timeout(Duration::from_secs(3), response(&mut stdout, 99))
        .await
        .expect("pending request must be answered before shutdown");
    assert_eq!(interrupted["error"]["code"], -32097);
    let reply = tokio::time::timeout(Duration::from_secs(3), response(&mut stdout, 2))
        .await
        .expect("shutdown must not wait for reconnect");
    assert!(reply.get("result").is_some_and(Value::is_null));
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
