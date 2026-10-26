/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Opt-in raw fixture records. Normal acceptance never writes evidence files.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Output;

use prod_code_mcp::extract_field::ExtractedField;
use serde_json::{Value, json};

fn enabled() -> bool {
    std::env::var_os("PROD_CODE_PRESERVATION_EVIDENCE").is_some()
}

pub fn emit(value: Value) {
    if enabled() {
        println!("PRESERVATION_EVIDENCE {}", value);
    }
}

fn sources(root: &Path) -> BTreeMap<String, String> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, files);
            } else if path.extension().is_some_and(|e| e == "rs" || e == "toml") {
                files.insert(
                    path.strip_prefix(root).unwrap().display().to_string(),
                    std::fs::read_to_string(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

pub fn operation(root: &Path, file: &Path, from: usize, to: usize, modes: Value) {
    if enabled() {
        let source = std::fs::read_to_string(file).unwrap();
        emit(json!({
            "kind": "operation", "sources": sources(root),
            "file": file.strip_prefix(root).unwrap(), "bytes": [from, to],
            "positions_utf16_1_based": [super::support::position(&source, from), super::support::position(&source, to)],
            "modes": modes,
        }));
    }
}

pub fn reply(root: &Path, method: &str, request: &Value, reply: Value) -> Value {
    if enabled() {
        // Locations are protocol fixtures, never native analyzer observations.
        let sanitize = |value: &Value| -> Value {
            serde_json::from_str(
                &serde_json::to_string(value)
                    .unwrap()
                    .replace(&root.display().to_string(), "<fixture>"),
            )
            .unwrap()
        };
        emit(json!({"kind": "scripted_reply", "method": method,
            "request": sanitize(request), "reply": sanitize(&reply)}));
    }
    reply
}

pub fn result(root: &Path, result: &ExtractedField) {
    if enabled() {
        let rewritten: BTreeMap<_, _> = result
            .rewritten
            .iter()
            .map(|(p, text)| (Path::new(p).strip_prefix(root).unwrap(), text))
            .collect();
        emit(json!({"kind": "returned", "sources": rewritten,
            "replaced": result.replaced, "constructors": result.constructors,
            "blocked": result.blocked, "unmatched": result.unmatched}));
    }
}

pub fn native(root: &Path, label: &str, kind: &str, argv: &[String], output: &Output) {
    if enabled() {
        emit(
            json!({"kind": kind, "label": label, "sources": sources(root),
            "argv": argv, "exit": output.status.code(), "success": output.status.success(),
            "stdout": String::from_utf8_lossy(&output.stdout).replace(&root.display().to_string(), "<fixture>"),
            "stderr": String::from_utf8_lossy(&output.stderr).replace(&root.display().to_string(), "<fixture>")}),
        );
    }
}
