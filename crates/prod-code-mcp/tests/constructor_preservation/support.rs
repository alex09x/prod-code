/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::replace_constructor::{
    ReplaceConstructorResult, ReplaceMode, replace_constructor_with_builder,
    replace_constructor_with_factory,
};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

pub fn run(root: &Path, argv: &[&str]) -> Value {
    let output = Command::new(argv[0])
        .args(&argv[1..])
        .current_dir(root)
        .output();
    match output {
        Ok(output) => json!({"argv": argv, "exit": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout).replace(&*root.to_string_lossy(), "<fixture>"),
            "stderr": String::from_utf8_lossy(&output.stderr).replace(&*root.to_string_lossy(), "<fixture>")}),
        Err(error) => json!({"argv": argv, "exit": null, "error": error.to_string()}),
    }
}

fn execute(root: &Path, ext: &str, edition: u32) -> Vec<Value> {
    match ext {
        "rs" => {
            let compile = run(
                root,
                &[
                    "rustc",
                    &format!("--edition={edition}"),
                    "main.rs",
                    "-o",
                    "program",
                ],
            );
            let mut results = vec![compile];
            if results[0]["exit"] == 0 {
                results.push(run(root, &["./program"]));
            }
            results
        }
        "cpp" => {
            let compile = run(root, &["c++", "-std=c++20", "main.cpp", "-o", "program"]);
            let mut results = vec![compile];
            if results[0]["exit"] == 0 {
                results.push(run(root, &["./program"]));
            }
            results
        }
        "mjs" => vec![run(root, &["node", "main.mjs"])],
        "ts" => vec![run(
            root,
            &["node", "--experimental-strip-types", "main.ts"],
        )],
        "py" => vec![run(root, &["python3", "main.py"])],
        "go" => vec![run(root, &["go", "run", "main.go"])],
        "swift" => reuse_swift(root),
        _ => panic!("unknown fixture language"),
    }
}

// The task carries native compiler evidence for these exact unchanged programs.
// Match bytes by hash before reusing it; never imply a fresh Swift invocation.
fn reuse_swift(root: &Path) -> Vec<Value> {
    let evidence: Value = serde_json::from_str(include_str!(
        "../../../../evidence/constructor-preservation/primary-swift-compiler.json"
    ))
    .unwrap();
    let source = fs::read_to_string(root.join("main.swift")).unwrap();
    let hash = bytes(root, &source)["sha256"].as_str().unwrap().to_string();
    let record = evidence["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["source_sha256"] == hash)
        .expect("Swift program changed; recorded native evidence cannot be reused");
    vec![
        json!({"argv":record["compiler_argv"],"exit":record["compiler_exit"],
        "stdout":record["compiler_stdout"],"stderr":record["compiler_stderr"],
        "reused_from":"primary-swift-compiler.json","source_sha256":hash}),
        json!({"argv":["<recorded-swift-program>"],"exit":record["runtime_exit"],
        "stdout":record["stdout"],"stderr":record["stderr"],
        "reused_from":"primary-swift-compiler.json","source_sha256":hash}),
    ]
}

pub fn record(id: &str, value: &Value) {
    if let Ok(directory) = std::env::var("CONSTRUCTOR_EVIDENCE") {
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            Path::new(&directory).join(format!("{id}.json")),
            serde_json::to_string_pretty(value).unwrap(),
        )
        .unwrap();
    }
}

pub fn bytes(root: &Path, text: &str) -> Value {
    fs::write(root.join("hash-input"), text).unwrap();
    let hash = run(root, &["sha256sum", "hash-input"]);
    json!({"utf8": text, "sha256": hash["stdout"].as_str().unwrap().split_whitespace().next().unwrap()})
}

pub async fn transform(
    ws: &Workspace,
    ext: &str,
    target: &str,
    mode: ReplaceMode,
    apply: bool,
) -> anyhow::Result<ReplaceConstructorResult> {
    let uri = format!("file://{}", ws.root().join(format!("main.{ext}")).display());
    let source = fs::read_to_string(ws.root().join(format!("main.{ext}"))).unwrap();
    let at = source.rfind(target).unwrap();
    let line = source[..at].bytes().filter(|b| *b == b'\n').count();
    let character = source[..at]
        .rsplit('\n')
        .next()
        .unwrap()
        .encode_utf16()
        .count();
    let target_len = target.len();
    let reference_uri = uri.clone();
    let texts = std::sync::Mutex::new(std::collections::BTreeMap::<String, String>::new());
    // Exact fixture reference, plus an explicit diagnostic sentinel. This script does NOT
    // certify generated code: before/after compiler and runtime results do that separately.
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/didOpen" => {
            texts.lock().unwrap().insert(params["textDocument"]["uri"].as_str().unwrap().into(),
                params["textDocument"]["text"].as_str().unwrap().into());
            Value::Null
        },
        "textDocument/didChange" => {
            if let Some(text) = params["contentChanges"][0]["text"].as_str() {
                texts.lock().unwrap().insert(params["textDocument"]["uri"].as_str().unwrap().into(), text.into());
            }
            Value::Null
        },
        "textDocument/references" => json!([{"uri": reference_uri,
            "range": {"start": {"line": line, "character": character}, "end": {"line": line, "character": character + target_len}}}]),
        "textDocument/diagnostic" => json!({"kind":"full", "items":[{
            "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
            "severity":1,"message":format!("scripted transport only, {} bytes; execute compiler separately",
                texts.lock().unwrap().get(params["textDocument"]["uri"].as_str().unwrap()).map_or(0, String::len))}]}),
        _ => Value::Null,
    }).await;
    let file = ws.root().join(format!("main.{ext}"));
    match mode {
        ReplaceMode::Factory => {
            replace_constructor_with_factory(
                gateway.addr(),
                &ws.root(),
                &file,
                target,
                None,
                apply,
                true,
                None,
            )
            .await
        }
        ReplaceMode::Builder => {
            replace_constructor_with_builder(
                gateway.addr(),
                &ws.root(),
                &file,
                target,
                None,
                apply,
                true,
                None,
            )
            .await
        }
    }
}

pub async fn check(
    id: &str,
    ext: &str,
    target: &str,
    source: &str,
    mode: ReplaceMode,
    count: usize,
    preserved: &[&str],
) -> String {
    check_with_edition(id, ext, target, source, mode, count, preserved, 2021).await
}

#[allow(clippy::too_many_arguments)]
pub async fn check_with_edition(
    id: &str,
    ext: &str,
    target: &str,
    source: &str,
    mode: ReplaceMode,
    count: usize,
    preserved: &[&str],
    edition: u32,
) -> String {
    let filename = format!("main.{ext}");
    let ws = Workspace::new(&[
        (filename.as_str(), source),
        (
            "Cargo.toml",
            &format!(
                "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\nedition=\"{edition}\"\n[[bin]]\nname=\"fixture\"\npath=\"main.rs\"\n"
            ),
        ),
    ]);
    let before = execute(&ws.root(), ext, edition);
    let result = transform(&ws, ext, target, mode, true).await;
    let after_source = fs::read_to_string(ws.root().join(&filename)).unwrap();
    let after = execute(&ws.root(), ext, edition);
    let input = bytes(&ws.root(), source);
    let output = bytes(&ws.root(), &after_source);
    record(
        id,
        &json!({"id": id, "mode": mode, "input":input, "output":output,
        "reference_fixture": {"file":filename, "line": source[..source.rfind(target).unwrap()].bytes().filter(|b| *b == b'\n').count(),
            "character":source[..source.rfind(target).unwrap()].rsplit('\n').next().unwrap().encode_utf16().count(), "length":target.len()},
        "scripted_transport": "exact reference fixture and nonempty diagnostic sentinel; force=true; no analyzer certification",
        "before":before, "after":after,
        "public_result": result.as_ref().map(|r| json!({"count":r.instantiations_rewritten,
            "blocked":r.blocked,"diagnostics":r.diagnostics})).unwrap_or_else(|e| json!({"error":format!("{e:#}")}))}),
    );
    if ext == "swift" && before[0]["exit"].is_null() {
        let result = result.unwrap();
        assert_eq!(result.instantiations_rewritten, count);
        assert!(
            after_source.contains("Point.create(x: 1)")
                || after_source.contains("PointBuilder().setX(1).build()")
        );
        return after_source;
    }
    assert!(
        before.iter().all(|r| r["exit"] == 0),
        "original {id}: {before:?}"
    );
    let result = result.unwrap_or_else(|e| panic!("{id}: {e:#}"));
    assert_eq!(result.instantiations_rewritten, count, "{id}");
    assert!(
        after.iter().all(|r| r["exit"] == 0),
        "rewritten {id}: {after:?}"
    );
    assert_eq!(
        before.last().unwrap()["stdout"],
        after.last().unwrap()["stdout"],
        "{id}"
    );
    for text in preserved {
        assert!(after_source.contains(text), "lost literal text {text:?}");
    }
    after_source
}
