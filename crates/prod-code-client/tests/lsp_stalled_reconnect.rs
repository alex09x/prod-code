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
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::BufReader;

#[tokio::test]
async fn stalled_reconnect_attempt_times_out_and_recovers() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn sample() {}\n")]);
    let cache = tempfile::tempdir().unwrap();
    let gateway = ScriptedGateway::start(|method, _| {
        if method == "textDocument/hover" {
            json!({"contents":"recovered"})
        } else {
            Value::Null
        }
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
    let uri = format!("file://{}/src/lib.rs", ws.root().display());
    write_frame(&mut stdin, &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"rust","version":1,"text":"pub fn sample() {}\n"}}}).to_string()).await.unwrap();
    down.send(2).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while blackholes.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("reconnect did not reach the stalled peer");
    down.send(0).unwrap();
    write_frame(&mut stdin, &json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":{"line":0,"character":3}}}).to_string()).await.unwrap();
    assert_eq!(
        response(&mut stdout, 2).await["result"]["contents"],
        "recovered"
    );
}
