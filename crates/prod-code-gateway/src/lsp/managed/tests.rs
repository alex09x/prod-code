/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use serde_json::json;

#[tokio::test]
async fn requests_unwrap_results_and_preserve_error_context_and_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let diagnostic = json!({"range":{"start":{"line":2,"character":0},"end":{"line":2,"character":1}},"message":"expected a value","severity":1});
    let engine = crate::lsp::test_support::server(
        temp.path(),
        json!({
            "test/value":{"result":[1,"value"]}, "test/missing":{},
            "test/error":{"error":{"message":"refused","code":-1}},
            "test/error-without-message":{"error":{"code":-1}},
            "textDocument/diagnostic":{"result":{"kind":"full","items":[diagnostic]}}
        }),
    )
    .await;
    let managed = ManagedLsp::Generic(&engine);
    assert_eq!(
        managed.request("test/value", json!({})).await.unwrap(),
        json!([1, "value"])
    );
    assert_eq!(
        managed.request("test/missing", json!({})).await.unwrap(),
        json!(null)
    );
    assert_eq!(
        managed
            .request("test/error", json!({}))
            .await
            .unwrap_err()
            .to_string(),
        "test/error: refused"
    );
    assert_eq!(
        managed
            .request("test/error-without-message", json!({}))
            .await
            .unwrap_err()
            .to_string(),
        "test/error-without-message: request failed"
    );
    assert_eq!(
        managed.diagnostics_for("file:///sample.py").await.unwrap(),
        vec![diagnostic]
    );
}
