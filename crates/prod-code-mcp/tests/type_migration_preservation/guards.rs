/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_mcp::type_migration::{migrate, migrate_ext};
use prod_code_testkit::answers;
use serde_json::json;
use std::fs;

const COPY: &str = "#[derive(Clone, Copy)]\npub struct Packet {\n    pub field: u32,\n}\n";

#[tokio::test]
async fn real_copy_derive_error_remains_authoritative() {
    let ws = workspace(COPY);
    let root = ws.root();
    let file = root.join("lib.rs");
    let (line, col) = position(COPY, "field");
    assert!(rustc(COPY).0);
    let changed = COPY.replace("field: u32", "field: String");
    let (ok, diagnostics) = rustc(&changed);
    assert!(!ok);
    println!("rustc Copy -> String: {diagnostics}");
    assert!(
        diagnostics["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "E0204" && d["range"]["start"]["line"] == 0)
    );
    let gw = gateway(json!([]), false, false).await;
    let plan = migrate(
        gw.server.addr(),
        &root,
        &file,
        line,
        col,
        "String",
        false,
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(fs::read(&file).unwrap(), COPY.as_bytes());
    let result = migrate(
        gw.server.addr(),
        &root,
        &file,
        line,
        col,
        "String",
        true,
        true,
        false,
    )
    .await;
    println!(
        "derive preview sites={} attributes={} report={} ordinary apply={result:?} bytes_unchanged={}",
        plan.sites.len(),
        plan.in_attributes,
        plan.render(20),
        fs::read(&file).unwrap() == COPY.as_bytes()
    );
    assert!(
        result.is_err(),
        "ordinary apply must reject a real rustc error"
    );
    assert_eq!(fs::read(&file).unwrap(), COPY.as_bytes());
    assert!(
        plan.sites
            .iter()
            .any(|s| s.code.as_deref() == Some("E0204"))
    );
    assert_eq!(plan.in_attributes, 1);
    assert!(!plan.render(20).contains("analyzer accepts the new type"));
    let forced = migrate(
        gw.server.addr(),
        &root,
        &file,
        line,
        col,
        "String",
        false,
        true,
        true,
    )
    .await
    .unwrap();
    assert!(forced.applied && !forced.sites.is_empty());
    assert_eq!(fs::read_to_string(&file).unwrap(), changed);
}

#[tokio::test]
async fn separately_displayed_derive_error_still_blocks() {
    let source = "#[derive(Clone)]\npub struct Packet {\n    pub field: u32,\n}\n";
    let ws = workspace(source);
    let root = ws.root();
    let file = root.join("lib.rs");
    let (line, col) = position(source, "field");
    let gw = gateway(json!([]), true, false).await;
    let plan = migrate(
        gw.server.addr(),
        &root,
        &file,
        line,
        col,
        "String",
        false,
        false,
        false,
    )
    .await
    .unwrap();
    let applied = migrate(
        gw.server.addr(),
        &root,
        &file,
        line,
        col,
        "String",
        false,
        true,
        false,
    )
    .await;
    println!("scripted in_derive preview={plan:?} apply={applied:?}");
    assert!(applied.is_err());
    assert_eq!(fs::read(&file).unwrap(), source.as_bytes());
    assert!(
        plan.sites
            .iter()
            .any(|s| s.code.as_deref() == Some("E0282"))
    );
    assert!(!plan.render(20).contains("analyzer accepts the new type"));
}

#[tokio::test]
async fn failed_reference_evidence_refuses_preview_apply_and_force() {
    let source = "pub fn value() -> u32 { 1 }\n";
    let caller = "pub fn caller() -> u32 { crate::value() }\n";
    let ws = workspace(source);
    ws.write("caller.rs", caller);
    let root = ws.root();
    let file = root.join("lib.rs");
    let (line, col) = position(source, "value");
    // Declaration diagnostics deliberately stay clean: unavailable references are
    // the only missing evidence, and must not become also_check = [].
    let cause = "scripted references unavailable: index crashed";
    assert!(rustc(&format!("{source}{caller}")).0);
    assert!(rustc(&source.replace("-> u32", "-> i64")).0);
    assert!(!rustc(&format!("{}{caller}", source.replace("-> u32", "-> i64"))).0);
    let gw = gateway(answers::failure(cause), false, true).await;
    let mut failures = Vec::new();
    for symbol_only in [false, true] {
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            ws.write("lib.rs", source);
            let result = if symbol_only {
                migrate_ext(
                    gw.server.addr(),
                    &root,
                    &file,
                    Some("value"),
                    None,
                    None,
                    "i64",
                    true,
                    false,
                    apply,
                    force,
                )
                .await
            } else {
                migrate(
                    gw.server.addr(),
                    &root,
                    &file,
                    line,
                    col,
                    "i64",
                    true,
                    apply,
                    force,
                )
                .await
            };
            let unchanged =
                fs::read(&file).unwrap() == source.as_bytes() && ws.read("caller.rs") == caller;
            println!(
                "reference failure symbol_only={symbol_only} apply={apply} force={force}: {result:?}; bytes_unchanged={unchanged}"
            );
            match result {
                Err(err) if format!("{err:#}").contains(cause) && unchanged => {}
                other => failures.push(format!(
                    "symbol_only={symbol_only} apply={apply} force={force}: {other:?}"
                )),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "missing reference evidence was accepted: {failures:?}"
    );
    assert!(
        !gw.requests
            .lock()
            .unwrap()
            .iter()
            .any(|(m, _)| m == "textDocument/diagnostic")
    );
}

#[tokio::test]
async fn malformed_references_are_not_an_empty_success() {
    let source = "pub fn value() -> u32 { 1 }\n";
    for reply in [
        json!({"wrong": []}),
        json!([{"uri": "file:///missing-range.rs"}]),
    ] {
        let ws = workspace(source);
        let root = ws.root();
        let file = root.join("lib.rs");
        let gw = gateway(reply, false, true).await;
        let result = migrate_ext(
            gw.server.addr(),
            &root,
            &file,
            Some("value"),
            None,
            None,
            "i64",
            false,
            false,
            true,
            true,
        )
        .await;
        assert!(
            result.is_err(),
            "malformed references cannot be forced: {result:?}"
        );
        assert_eq!(fs::read(&file).unwrap(), source.as_bytes());
    }
}

#[tokio::test]
async fn discovered_external_error_blocks_and_null_references_can_be_clean() {
    let source = "pub fn value() -> u32 { 1 }\n";
    let ws = workspace(source);
    let caller = "pub fn caller() -> u32 { crate::value() }\n";
    ws.write("caller.rs", caller);
    let root = ws.root();
    let file = root.join("lib.rs");
    let uri = url::Url::from_file_path(root.join("caller.rs"))
        .unwrap()
        .to_string();
    let references = json!([{"uri": uri, "range": {"start": {"line": 0, "character": 31}}}]);
    // The external caller's type error is scripted independently of declaration
    // diagnostics. Both files must be opened, so an empty also_check cannot pass.
    let proposed = std::sync::atomic::AtomicBool::new(false);
    let gw = prod_code_testkit::ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => references.clone(),
        "textDocument/didOpen" | "textDocument/didChange" => {
            let text = params["textDocument"]["text"]
                .as_str()
                .or_else(|| params["contentChanges"][0]["text"].as_str())
                .unwrap_or("");
            if text.contains("pub fn value()") {
                proposed.store(
                    text.contains("-> i64"),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            serde_json::Value::Null
        }
        "textDocument/diagnostic"
            if params["textDocument"]["uri"] == uri
                && proposed.load(std::sync::atomic::Ordering::Relaxed) =>
        {
            answers::error_at(1, 32, "E0308", "expected u32, found i64")
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/documentSymbol" => json!([]),
        _ => serde_json::Value::Null,
    })
    .await;
    let (line, col) = position(source, "value");
    let result = migrate(
        gw.addr(),
        &root,
        &file,
        line,
        col,
        "i64",
        false,
        true,
        false,
    )
    .await;
    assert!(
        result.is_err(),
        "external errors must block apply: {result:?}"
    );
    assert_eq!(fs::read(&file).unwrap(), source.as_bytes());
    assert_eq!(ws.read("caller.rs"), caller);

    let ws = workspace(source);
    let root = ws.root();
    let file = root.join("lib.rs");
    let clean = gateway(serde_json::Value::Null, false, false).await;
    let applied = migrate_ext(
        clean.server.addr(),
        &root,
        &file,
        Some("value"),
        None,
        None,
        "i64",
        false,
        false,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(applied.applied && applied.sites.is_empty());
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        source.replace("-> u32", "-> i64")
    );
}
