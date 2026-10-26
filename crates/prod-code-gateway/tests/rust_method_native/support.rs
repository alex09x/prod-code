/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::native::{Fixture, bridge::Answer};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, process::Command, sync::OnceLock};

fn artifact_root() -> &'static PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let tracked = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../evidence/issue-1357/native-consumer");
        if let Ok(explicit) = std::env::var("PROD_CODE_NATIVE_ARTIFACT_DIR") {
            let trimmed = explicit.trim();
            if !trimmed.is_empty() {
                let path = PathBuf::from(trimmed);
                std::fs::create_dir_all(&path).unwrap();
                let tracked_canon = std::fs::canonicalize(&tracked).unwrap_or(tracked);
                let path_canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                assert_ne!(
                    path_canon, tracked_canon,
                    "PROD_CODE_NATIVE_ARTIFACT_DIR must not overwrite tracked historical evidence"
                );
                return path;
            }
        }
        tempfile::tempdir().unwrap().keep()
    })
}

pub fn artifact(label: &str, name: &str, text: &str) {
    let dir = artifact_root().join(label);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), text).unwrap();
}

pub fn position(source: &str, needle: &str) -> (u32, u32) {
    let prefix = &source[..source.find(needle).unwrap()];
    (
        prefix.bytes().filter(|&b| b == b'\n').count() as u32 + 1,
        prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32 + 1,
    )
}

pub fn runtime(label: &str, phase: &str, source: &str) -> String {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("main.rs");
    let bin = temp.path().join("main");
    std::fs::write(&file, source).unwrap();
    let compiled = Command::new("rustc")
        .args([
            "--edition=2021",
            "-C",
            "overflow-checks=yes",
            "--error-format=short",
        ])
        .arg(&file)
        .arg("-o")
        .arg(&bin)
        .output()
        .unwrap();
    let compile = format!(
        "exit={}\nstdout={}\nstderr={}\n",
        compiled.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    artifact(label, &format!("{phase}-compiler.txt"), &compile);
    assert!(compiled.status.success(), "{compile}");
    let ran = Command::new(bin).output().unwrap();
    let output = format!(
        "exit={}\nstdout={}\nstderr={}\n",
        ran.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    artifact(label, &format!("{phase}-runtime.txt"), &output);
    assert!(ran.status.success(), "{output}");
    assert!(ran.stderr.is_empty(), "{output}");
    String::from_utf8(ran.stdout).unwrap()
}

pub fn record_answers(label: &str, answers: &[Answer]) {
    let records: Vec<Value> = answers
        .iter()
        .map(|a| {
            json!({
                "session": a.session, "validation": a.validation, "method": a.method,
                "request": a.request, "response": a.response,
            })
        })
        .collect();
    artifact(
        label,
        "semantic-replies.json",
        &serde_json::to_string_pretty(&records).unwrap(),
    );
}

pub fn require_methods(answers: &[Answer], methods: &[&str]) {
    for method in methods {
        assert!(
            answers.iter().any(|a| a.method == *method),
            "public consumer never requested {method}"
        );
    }
}

pub fn dispatch_identity(answers: &[Answer], rewritten: &str, calls: usize) {
    let (line, col) = position(rewritten, "f(");
    let mut call_positions = BTreeSet::new();
    let matching = answers
        .iter()
        .filter(|a| {
            if !a.validation || a.method != "textDocument/definition" {
                return false;
            }
            let Some(locations) = a.response["result"].as_array() else {
                return false;
            };
            let matches = locations.len() == 1
                && locations[0]["range"]["start"] == json!({"line":line-1,"character":col-1})
                && locations[0]["uri"] == a.request["params"]["textDocument"]["uri"];
            if matches {
                if let (Some(req_line), Some(req_col)) = (
                    a.request["params"]["position"]["line"].as_u64(),
                    a.request["params"]["position"]["character"].as_u64(),
                ) {
                    call_positions.insert((req_line, req_col));
                }
            }
            matches
        })
        .count();
    assert!(
        matching >= calls && call_positions.len() == calls,
        "dot calls lack distinct native validation definition identities (matching={matching}, distinct={}): {answers:#?}",
        call_positions.len()
    );
}

// These observations are control probes, not semantic input to convert_to_method.
// Public traffic is retained separately by Bridge and must satisfy its own assertions.
pub async fn observe_probes(
    label: &str,
    fixture: &Fixture,
    source: &str,
    decl_needle: &str,
    decl_offset: u32,
    impl_needle: &str,
    impl_offset: u32,
) {
    let mut session = fixture.session(1).await;
    // This direct session translates /client; use its public virtual URI.
    let (line, col) = position(source, decl_needle);
    let params = json!({"textDocument":{"uri":"file:///client/src/lib.rs"},
        "position":{"line":line-1,"character":col+decl_offset}});
    let mut records = Vec::new();
    for method in [
        "textDocument/definition",
        "textDocument/hover",
        "textDocument/typeDefinition",
    ] {
        let reply = session.request(json!(method), method, params.clone()).await;
        records.push(json!({"method":method,"params":params,"response":reply}));
    }
    // Probe the exact impl-owner position used by the consumer, rather than infer
    // type-navigation behavior from a declaration name or a display string.
    let (line, col) = position(source, impl_needle);
    let params = json!({"textDocument":{"uri":"file:///client/src/lib.rs"},
        "position":{"line":line-1,"character":col+impl_offset}});
    for method in ["textDocument/definition", "textDocument/typeDefinition"] {
        let reply = session.request(json!(method), method, params.clone()).await;
        records.push(json!({"method":method,"params":params,"response":reply}));
    }
    artifact(
        label,
        "control-probes.json",
        &serde_json::to_string_pretty(&records).unwrap(),
    );
    fixture.finish(vec![session]).await;
}

pub async fn observe(fixture: &Fixture, source: &str) {
    let label = if source.contains("identity!") {
        "type-capture"
    } else if source.contains("fixed!") {
        "local-value"
    } else {
        "nominal"
    };
    observe_probes(label, fixture, source, "struct S", 6, "impl S", 4).await;
}
