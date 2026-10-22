/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;

use super::{validate_text, validate_texts};

const ORIGINAL: &str = "fn run() {\n    let accumulator = 0;\n    let result = accumulator;\n    let legacy = old_missing;\n}\n";
const PROPOSED: &str =
    "fn run() {\n\n    let result = accumulator;\n    let legacy = old_missing;\n}\n";

async fn gateway(ws: &Workspace) -> ScriptedGateway {
    let root = ws.root();
    let opened = Mutex::new(HashMap::<String, String>::new());
    ScriptedGateway::start(move |method, params| {
        let mut opened = opened.lock().unwrap();
        if method == "prod-code/handshake" {
            opened.clear();
        }
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
        if method == "textDocument/didOpen" {
            opened.insert(
                uri.into(),
                params["textDocument"]["text"].as_str().unwrap().into(),
            );
        }
        if method == "textDocument/diagnostic" {
            let file = uri.rsplit('/').next().unwrap();
            let text = opened
                .get(uri)
                .cloned()
                .unwrap_or_else(|| std::fs::read_to_string(root.join(file)).unwrap());
            let mut items =
                answers::error_at(4, 18, "E0425", "cannot find value `old_missing`")["items"]
                    .as_array()
                    .unwrap()
                    .clone();
            if !text.contains("let accumulator =") {
                items.extend(
                    answers::error_at(3, 18, "E0425", "cannot find value `accumulator`")["items"]
                        .as_array()
                        .unwrap()
                        .clone(),
                );
            }
            return json!({"kind": "full", "items": items});
        }
        json!([])
    })
    .await
}

fn assert_new_error(report: &crate::diagnostics::DiagnosticsReport) {
    assert_eq!(report.errors, 1, "{report:?}");
    assert_eq!(report.items[0].code.as_deref(), Some("E0425"));
    assert!(report.items[0].message.contains("accumulator"));
    assert_eq!(report.preexisting.len(), 1, "{report:?}");
    assert!(report.preexisting[0].message.contains("old_missing"));
    assert!(!report.ok());
}

#[tokio::test]
async fn validate_single_already_written_uses_committed_baseline() {
    let ws = Workspace::new(&[("a.rs", ORIGINAL)]);
    ws.write("a.rs", PROPOSED);
    let gateway = gateway(&ws).await;
    let report = validate_text(gateway.addr(), &ws.root(), &ws.path("a.rs"), PROPOSED)
        .await
        .unwrap();
    assert_new_error(&report);
    assert_eq!(ws.read("a.rs"), PROPOSED);
}

#[tokio::test]
async fn validate_batch_already_written_uses_every_committed_baseline() {
    let ws = Workspace::new(&[("a.rs", ORIGINAL), ("b.rs", ORIGINAL)]);
    ws.write("a.rs", PROPOSED);
    ws.write("b.rs", PROPOSED);
    let gateway = gateway(&ws).await;
    let reports = validate_texts(
        gateway.addr(),
        &ws.root(),
        &[
            ("a.rs".into(), PROPOSED.into()),
            (ws.path("b.rs"), PROPOSED.into()),
        ],
        &[],
    )
    .await
    .unwrap();
    assert_eq!(reports.len(), 2);
    for report in &reports {
        assert_new_error(report);
    }
    assert_eq!(ws.read("a.rs"), PROPOSED);
    assert_eq!(ws.read("b.rs"), PROPOSED);
}

#[tokio::test]
async fn validate_batch_opens_all_baselines_before_diagnostics() {
    const CALLER: &str = "fn run() { let result = crate::b::VALUE; }\n";
    let ws = Workspace::new(&[("a.rs", CALLER), ("b.rs", "pub const VALUE: u32 = 0;\n")]);
    ws.write("b.rs", "// VALUE removed\n");
    let root = ws.root();
    let opened = Mutex::new(HashMap::<String, String>::new());
    let gateway = ScriptedGateway::start(move |method, params| {
        let mut opened = opened.lock().unwrap();
        if method == "prod-code/handshake" {
            opened.clear();
        }
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
        if method == "textDocument/didOpen" {
            opened.insert(
                uri.into(),
                params["textDocument"]["text"].as_str().unwrap().into(),
            );
        }
        if method == "textDocument/diagnostic" && uri.ends_with("/a.rs") {
            let b = opened
                .iter()
                .find(|(uri, _)| uri.ends_with("/b.rs"))
                .map(|(_, text)| text.clone())
                .unwrap_or_else(|| std::fs::read_to_string(root.join("b.rs")).unwrap());
            if !b.contains("pub const VALUE") {
                return answers::error_at(1, 39, "E0425", "cannot find value `VALUE`");
            }
        }
        if method == "textDocument/diagnostic" {
            return answers::no_diagnostics();
        }
        json!([])
    })
    .await;
    let reports = validate_texts(
        gateway.addr(),
        &ws.root(),
        &[
            ("a.rs".into(), CALLER.into()),
            ("b.rs".into(), "// VALUE removed\n".into()),
        ],
        &[],
    )
    .await
    .unwrap();
    assert_eq!(reports[0].errors, 1, "{:?}", reports[0]);
    assert!(reports[0].preexisting.is_empty());
}
