/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::json;

use crate::config::{GoConfig, find_gopls_binary};
use crate::engine::GoEngine;
use crate::lsp::normalize_diagnostic_response;

#[test]
fn only_complete_empty_kind_diagnostic_reports_are_normalized() {
    for items in [json!([]), json!([{"message":"an error","severity":1}])] {
        let mut reply =
            json!({"jsonrpc":"2.0","id":4,"result":{"kind":"","items":items,"resultId":"keep"}});
        normalize_diagnostic_response("textDocument/diagnostic", &mut reply);
        assert_eq!(reply["result"]["kind"], "full");
        assert_eq!(reply["result"]["items"], items);
        assert_eq!(reply["result"]["resultId"], "keep");
        assert_eq!(reply["id"], 4);
    }
    for original in [
        json!(null),
        json!({"result":null}),
        json!({"result":{"kind":""}}),
        json!({"result":{"kind":"","items":null}}),
        json!({"result":{"kind":"","items":{}}}),
        json!({"result":{"kind":"unchanged","resultId":"old","items":[]}}),
        json!({"result":{"kind":"mystery","items":[]}}),
        json!({"error":{"code":-32603},"result":{"kind":"","items":[]}}),
    ] {
        let mut reply = original.clone();
        normalize_diagnostic_response("textDocument/diagnostic", &mut reply);
        assert_eq!(reply, original);
    }
    let original = json!({"result":{"kind":"","items":[]}});
    let mut reply = original.clone();
    normalize_diagnostic_response("textDocument/hover", &mut reply);
    assert_eq!(reply, original);
}

#[tokio::test]
async fn real_gopls_diagnostics_are_full_and_preserve_actual_errors() {
    let gopls = match find_gopls_binary(None) {
        Some(bin) => bin,
        None => {
            eprintln!(
                "SKIPPED real_gopls_diagnostics_are_full_and_preserve_actual_errors: gopls not found"
            );
            return;
        }
    };
    let dir = tempfile::Builder::new()
        .prefix("go-diagnostic-report-")
        .tempdir()
        .unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let source = "package subject\n\nfunc Value() int { return 1 }\n";
    std::fs::write(
        root.join("go.mod"),
        "module example.com/diagnostics\n\ngo 1.22\n",
    )
    .unwrap();
    std::fs::write(root.join("value.go"), source).unwrap();
    let engine = GoEngine::load(
        &root,
        GoConfig {
            gopls_path: Some(gopls),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let uri = url::Url::from_file_path(root.join("value.go"))
        .unwrap()
        .to_string();
    engine.did_open(&uri, source).await.unwrap();
    let ready = engine
        .send_request("workspace/symbol", json!({"query":"Value"}))
        .await
        .unwrap();
    assert!(!ready["result"].as_array().unwrap().is_empty(), "{ready}");
    for (version, text, error) in [
        (
            2,
            "package subject\n\nfunc Value() int { return \"bad\" }\n",
            true,
        ),
        (3, source, false),
    ] {
        engine.did_change(&uri, text, version).await.unwrap();
        let answer = engine
            .send_request(
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
            )
            .await
            .unwrap();
        eprintln!("version {version}: {answer}");
        assert!(answer.get("error").is_none(), "{answer}");
        assert_eq!(answer["result"]["kind"], "full", "{answer}");
        let items = answer["result"]["items"].as_array().unwrap();
        assert_eq!(items.iter().any(|d| d["severity"] == 1), error, "{answer}");
        if error {
            assert!(
                items.iter().any(|d| d["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("string") && m.contains("int"))),
                "{answer}"
            );
        }
    }
}
