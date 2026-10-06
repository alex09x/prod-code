/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Requests for different workspaces must not share a network-operation mutex (#430).
use prod_code_mcp::session::pooled_query;
use prod_code_testkit::{ScriptedGateway, Workspace};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_workspace_does_not_block_another_workspace() {
    let first = Workspace::new(&[("src/lib.rs", "pub fn first() {}\n")]);
    let second = Workspace::new(&[("src/lib.rs", "pub fn second() {}\n")]);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (notify, gate) = (Arc::clone(&entered), Arc::clone(&release));
    let slow = ScriptedGateway::start(move |method, _| {
        if method == "textDocument/documentSymbol" {
            notify.notify_one();
            let (lock, wake) = &*gate;
            tokio::task::block_in_place(|| {
                let _held = wake
                    .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(3), |done| !*done)
                    .unwrap();
            });
        }
        serde_json::json!([])
    })
    .await;
    let fast = ScriptedGateway::start(|_, _| serde_json::json!(["fast"])).await;
    let root = first.root();
    let file = first.path("src/lib.rs");
    let waiting = tokio::spawn(async move {
        pooled_query(
            slow.addr(),
            &root,
            &file,
            "textDocument/documentSymbol",
            serde_json::json!({}),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("slow query reached server");
    let answer = tokio::time::timeout(
        Duration::from_millis(500),
        pooled_query(
            fast.addr(),
            &second.root(),
            &second.path("src/lib.rs"),
            "textDocument/documentSymbol",
            serde_json::json!({}),
        ),
    )
    .await;
    let (lock, wake) = &*release;
    *lock.lock().unwrap() = true;
    wake.notify_all();
    waiting.await.unwrap().unwrap();
    assert_eq!(
        answer
            .expect("an unrelated workspace must answer while the first is blocked")
            .unwrap(),
        serde_json::json!(["fast"])
    );
}
