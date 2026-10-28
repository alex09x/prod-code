/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::pool::pooled_query;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pooled_query_reopens_after_workspace_model_change_disconnect() {
    let workspace = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let attempts = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&attempts);
    let gateway = prod_code_testkit::ScriptedGateway::start(move |method, _| {
        if method == "textDocument/documentSymbol" && counted.fetch_add(1, Ordering::Relaxed) == 0 {
            serde_json::json!({
                "disconnect": prod_code_protocol::WORKSPACE_MODEL_CHANGED_REASON,
            })
        } else {
            serde_json::json!([])
        }
    })
    .await;

    let result = pooled_query(
        gateway.addr(),
        &workspace.root(),
        &workspace.path("src/lib.rs"),
        "textDocument/documentSymbol",
        serde_json::json!({}),
    )
    .await;

    assert_eq!(result.unwrap(), serde_json::json!([]));
    assert_eq!(attempts.load(Ordering::Relaxed), 2);
    assert_eq!(gateway.calls(), 2);
}
