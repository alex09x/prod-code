/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1814: parallel calls on one checkout share one pooled session. A call whose budget ends
//! while it waits for that session says it was queued, not that the analyzer was busy.

use super::super::pool::{pool_key, pooled_query, session_slot};
use prod_code_testkit::{ScriptedGateway, Workspace};
use std::time::Duration;

const CARGO: &str = "[package]\nname = \"issue_1814\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_1814_a_call_queued_behind_another_query_says_it_was_queued() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", "pub fn target() {}\n"),
    ]);
    let gateway = ScriptedGateway::start(|_, _| serde_json::json!([])).await;
    let (root, file) = (ws.root(), ws.path("src/lib.rs"));
    pooled_query(
        gateway.addr(),
        &root,
        &file,
        "textDocument/documentSymbol",
        serde_json::json!({}),
    )
    .await
    .unwrap();
    let asked = gateway.calls();
    // Another call owns the checkout's session for longer than this call's budget.
    let (_, key) = pool_key(gateway.addr(), &root, &file);
    let slot = session_slot(&key).await;
    let held = slot.lock().await;

    let args = serde_json::json!({
        "path": "src/lib.rs", "line": 1, "character": 8, "body": false, "budget_ms": 300
    });
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        crate::tools::execute_tool(gateway.addr(), &root, "code_definition", args),
    )
    .await
    .expect("the budget ends the wait")
    .expect("a busy answer, not a failure");
    let text = format!("{:?}", result.content);
    assert!(result.is_error, "{text}");
    assert!(
        text.contains("queued") && text.contains("another query"),
        "the answer must say the call waited for another query: {text}"
    );
    assert_eq!(
        gateway.calls(),
        asked,
        "the queued call never reached the analyzer"
    );
    drop(held);
}
