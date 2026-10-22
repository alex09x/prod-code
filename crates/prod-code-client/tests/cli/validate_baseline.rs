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
use std::sync::Mutex;

const ORIGINAL: &str = "pub fn run() {\n    let accumulator = 0;\n    let result = accumulator;\n    let legacy = old_missing;\n}\n";
const PROPOSED: &str =
    "pub fn run() {\n\n    let result = accumulator;\n    let legacy = old_missing;\n}\n";

async fn validate_written(with: bool) {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", ORIGINAL),
        ("src/other.rs", ORIGINAL),
    ]);
    ws.write("src/lib.rs", PROPOSED);
    ws.write("src/other.rs", PROPOSED);
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
        if method == "textDocument/diagnostic" {
            let file = uri.rsplit('/').next().unwrap();
            let text = opened
                .get(uri)
                .cloned()
                .unwrap_or_else(|| std::fs::read_to_string(root.join("src").join(file)).unwrap());
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
            return serde_json::json!({"kind": "full", "items": items});
        }
        serde_json::json!([])
    })
    .await;
    let mut args = vec!["validate", "src/lib.rs", "--from", "src/lib.rs", "--json"];
    if with {
        args.extend(["--with", "src/other.rs=src/other.rs"]);
    }
    let out = run_cli(&ws, gateway.addr(), &args).await;
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {} stderr: {}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let reports = if with {
        value.as_array().unwrap().clone()
    } else {
        vec![value]
    };
    assert_eq!(reports.len(), if with { 2 } else { 1 });
    for report in reports {
        assert_eq!(report["errors"], 1, "{report}");
        assert!(
            report["items"][0]["message"]
                .as_str()
                .unwrap()
                .contains("accumulator")
        );
        assert_eq!(report["preexisting"].as_array().unwrap().len(), 1);
        assert!(
            report["preexisting"][0]["message"]
                .as_str()
                .unwrap()
                .contains("old_missing")
        );
    }
    assert_eq!(ws.read("src/lib.rs"), PROPOSED);
    assert_eq!(ws.read("src/other.rs"), PROPOSED);
}

#[tokio::test]
async fn cli_validate_from_self_reports_new_error() {
    validate_written(false).await;
}

#[tokio::test]
async fn cli_validate_with_self_reports_new_errors_for_both_files() {
    validate_written(true).await;
}
