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

#[tokio::test]
async fn cancellation_sent_before_disconnect_still_gets_a_response() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn sample() {}\n")]);
    let cache = tempfile::tempdir().unwrap();
    let canceled = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&canceled);
    let gateway = ScriptedGateway::start(move |method, _| {
        if method == "$/cancelRequest" {
            observed.store(true, Ordering::Release);
        }
        if method == "textDocument/hover" {
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
        &json!({"jsonrpc":"2.0","id":8,"method":"textDocument/hover","params":{}}).to_string(),
    )
    .await
    .unwrap();
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":8}}).to_string(),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !canceled.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("gateway did not observe cancellation");
    down.send(1).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    down.send(0).unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(8), response(&mut stdout, 8))
        .await
        .expect("canceled request must receive a response");
    assert_eq!(reply["error"]["code"], -32800);
}
