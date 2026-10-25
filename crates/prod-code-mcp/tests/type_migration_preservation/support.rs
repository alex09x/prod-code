/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::signature::position_at;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::{collections::HashMap, fs, process::Command, sync::Arc, sync::Mutex};

pub fn workspace(source: &str) -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"preserve\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        ),
        ("lib.rs", source),
    ])
}

pub fn position(source: &str, name: &str) -> (u32, u32) {
    position_at(source, source.find(name).unwrap()).unwrap()
}

// This is a protocol adapter, not an analyzer simulation: every error and primary
// span comes from rustc compiling exactly the currently opened overlay bytes.
// Expanded diagnostics use rustc's recorded invocation span in user source.
pub fn rustc(source: &str) -> (bool, Value) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("lib.rs"), source).unwrap();
    let out = Command::new("rustc")
        .current_dir(dir.path())
        .args([
            "--edition=2024",
            "--crate-type=lib",
            "--emit=metadata",
            "--error-format=json",
            "-Awarnings",
            "lib.rs",
        ])
        .output()
        .expect("rustc must be available on the build node");
    let mut items = Vec::new();
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        let d: Value = serde_json::from_str(line).expect("rustc JSON");
        if d["level"] != "error" {
            continue;
        }
        for span in d["spans"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["is_primary"] == true)
        {
            let span = span
                .get("expansion")
                .and_then(|e| e.get("span"))
                .filter(|s| s["file_name"] == "lib.rs")
                .unwrap_or(span);
            let start = span["byte_start"].as_u64().unwrap() as usize;
            let end = span["byte_end"].as_u64().unwrap() as usize;
            let (line, col) = position_at(source, start).unwrap();
            let (el, ec) = position_at(source, end).unwrap();
            let label = span["label"].as_str().unwrap_or("");
            // Match rust-analyzer's one-line mismatch wording for conversion suggestions.
            let message = if label.starts_with("expected ") && label.contains(", found ") {
                label.replace('`', "")
            } else {
                d["message"].as_str().unwrap().to_string()
            };
            items.push(
                json!({"severity": 1, "code": d["code"]["code"], "message": message,
                "range": {"start": {"line": line - 1, "character": col - 1},
                          "end": {"line": el - 1, "character": ec - 1}}}),
            );
        }
    }
    if !out.status.success() {
        assert!(
            !items.is_empty(),
            "rustc failed without primary errors: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    (
        out.status.success(),
        json!({"kind": "full", "items": items}),
    )
}

pub struct Gateway {
    pub server: ScriptedGateway,
    pub requests: Arc<Mutex<Vec<(String, Value)>>>,
}

// References and explicit failure modes are scripted; normal diagnostics use rustc.
// The separated E0282 mode exercises the existing diagnostics in_derive bucket.
pub async fn gateway(references: Value, separated: bool, clean: bool) -> Gateway {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let texts = Mutex::new(HashMap::<String, String>::new());
    let server = ScriptedGateway::start(move |method, params| {
        recorded
            .lock()
            .unwrap()
            .push((method.into(), params.clone()));
        match method {
            "textDocument/references" => references.clone(),
            "textDocument/didOpen" | "textDocument/didChange" => {
                let text = params["textDocument"]["text"]
                    .as_str()
                    .or_else(|| params["contentChanges"][0]["text"].as_str());
                if let Some(text) = text {
                    texts.lock().unwrap().insert(
                        params["textDocument"]["uri"].as_str().unwrap().into(),
                        text.into(),
                    );
                }
                Value::Null
            }
            "textDocument/documentSymbol" => json!([]),
            "textDocument/diagnostic" => {
                if clean {
                    return answers::no_diagnostics();
                }
                let text =
                    texts.lock().unwrap()[params["textDocument"]["uri"].as_str().unwrap()].clone();
                if separated {
                    return if text.contains("field: String") {
                        answers::error_at(1, 10, "E0282", "scripted derive inference error")
                    } else {
                        answers::no_diagnostics()
                    };
                }
                rustc(&text).1
            }
            _ => Value::Null,
        }
    })
    .await;
    Gateway { server, requests }
}
