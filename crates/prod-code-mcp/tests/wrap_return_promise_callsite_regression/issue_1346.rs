/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{gateway, tool, workspace};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;

const SELECTED: &str = "export function retry() { return 1; }\n";

async fn apply_fallback(caller_files: &[(&str, &str)]) -> (Workspace, Result<String, String>) {
    let ws = workspace(SELECTED, "js");
    for (path, source) in caller_files {
        fs::write(ws.root().join(path), source).unwrap();
    }
    let (remote, _) = gateway(&ws, SELECTED, &[], "js").await;
    let result = tool(remote.addr(), &ws, "js", true, false).await;
    (ws, result)
}

fn locations(path: &Path, source: &str, needle: &str) -> Value {
    let at = source.find(needle).expect(needle);
    let line = source[..at].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let line_start = source[..at].rfind('\n').map_or(0, |index| index + 1);
    let column = source[line_start..at].encode_utf16().count() as u32 + 1;
    answers::locations(path, &[(line, column)])
}

async fn semantic_gateway(references: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => references.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
}

#[tokio::test]
async fn commonjs_namespace_require_rewrites_the_selected_member_call() {
    let selected = "function retry() { return 1; }\nmodule.exports = { retry };\n";
    let caller = "const math = require('./selected.js');\nasync function run() { return math.retry(); }\nrun().then(value => { if (value !== 1) throw new Error('wrong result'); });\n";
    let ws = workspace(selected, "js");
    fs::write(ws.root().join("package.json"), "{\"type\":\"commonjs\"}\n").unwrap();
    fs::write(ws.root().join("caller.js"), caller).unwrap();
    let (remote, _) = gateway(&ws, selected, &[], "js").await;

    let result = tool(remote.addr(), &ws, "js", true, false).await;
    assert!(result.is_ok(), "{}", result.unwrap_err());
    let rewritten = ws.read("caller.js");
    assert!(
        rewritten.contains("await math.retry()"),
        "{result:?}\n{rewritten}"
    );

    let output = Command::new("node")
        .arg(ws.path("caller.js"))
        .output()
        .expect("run CommonJS caller control");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn dynamic_import_namespace_is_proven_in_empty_reference_fallback() {
    let caller = "async function run() {\n  const math = await import('./selected.js');\n  return math.retry();\n}\nrun().then(value => { if (value !== 1) throw new Error('wrong result'); });\n";
    let (ws, result) = apply_fallback(&[("caller.js", caller)]).await;
    assert!(result.is_ok(), "{}", result.unwrap_err());
    let rewritten = ws.read("caller.js");
    assert!(rewritten.contains("await math.retry()"), "{rewritten}");
    let output = Command::new("node")
        .arg(ws.path("caller.js"))
        .output()
        .expect("run dynamic-import caller control");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn dynamic_import_destructuring_rewrites_the_local_export_alias() {
    let caller = "const { retry: localRetry } = await import('./selected.js');\nasync function run() { return localRetry(); }\nrun().then(value => { if (value !== 1) throw new Error('wrong result'); });\n";
    let (ws, result) = apply_fallback(&[("caller.js", caller)]).await;
    assert!(result.is_ok(), "{}", result.unwrap_err());
    let rewritten = ws.read("caller.js");
    assert!(
        rewritten.contains("return await localRetry();"),
        "{rewritten}"
    );
    let output = Command::new("node")
        .arg(ws.path("caller.js"))
        .output()
        .expect("run dynamic-import alias control");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn same_line_named_import_alias_is_rewritten_with_semantic_references() {
    let caller = "import { retry as again } from './selected.js'; async function run() { return again(); }\n";
    let ws = workspace(SELECTED, "js");
    let caller_path = ws.path("caller.js");
    fs::write(&caller_path, caller).unwrap();
    let references = locations(&caller_path, caller, "again()");
    let remote = semantic_gateway(references).await;
    let result = tool(remote.addr(), &ws, "js", true, false).await;
    assert!(result.is_ok(), "{}", result.unwrap_err());
    let rewritten = ws.read("caller.js");
    assert!(rewritten.contains("return await again();"), "{rewritten}");
}

#[tokio::test]
async fn later_hoisted_local_function_is_not_awaited_as_an_import() {
    let caller = "import { retry } from './selected.js';\nasync function run() { return retry(); function retry() { return 0; } }\n";
    let (ws, result) = apply_fallback(&[("caller.js", caller)]).await;
    assert!(result.is_ok(), "{}", result.unwrap_err());
    assert_eq!(ws.read("caller.js"), caller);
}

#[tokio::test]
async fn closed_initializer_arrow_does_not_shadow_an_outer_import() {
    let caller = "import { retry } from './selected.js';\nconst setup = () => {\n  const retry = () => 0;\n  return retry();\n};\nasync function run() { return retry(); }\n";
    let (ws, result) = apply_fallback(&[("caller.js", caller)]).await;
    assert!(result.is_ok(), "{}", result.unwrap_err());
    let rewritten = ws.read("caller.js");
    assert!(rewritten.contains("  return retry();\n};"), "{rewritten}");
    assert!(rewritten.contains("return await retry();"), "{rewritten}");
}

#[tokio::test]
async fn local_commonjs_property_alias_keeps_its_import_proof() {
    let selected = "function retry() { return 1; }\nmodule.exports = { retry };\n";
    let caller = "async function run() {\n  const localRetry = require('./selected.js').retry;\n  return localRetry();\n}\n";
    let ws = workspace(selected, "js");
    fs::write(ws.root().join("package.json"), "{\"type\":\"commonjs\"}\n").unwrap();
    fs::write(ws.root().join("caller.js"), caller).unwrap();
    let (remote, _) = gateway(&ws, selected, &[], "js").await;
    let result = tool(remote.addr(), &ws, "js", true, false).await;
    assert!(result.is_ok(), "{}", result.unwrap_err());
    let rewritten = ws.read("caller.js");
    assert!(
        rewritten.contains("return await localRetry();"),
        "{rewritten}"
    );
}

#[tokio::test]
async fn nested_local_commonjs_namespace_still_shadows_the_import() {
    let caller = "const math = require('./selected.js');\nasync function run(other) { const math = other; return math.retry(); }\n";
    let (ws, result) = apply_fallback(&[("caller.js", caller)]).await;
    assert!(result.is_ok(), "{}", result.unwrap_err());
    assert_eq!(ws.read("caller.js"), caller);
}

#[tokio::test]
async fn expression_arrow_parameter_binding_is_not_unmatched_call_evidence() {
    let caller = "import { retry } from './selected.js';\nconst callback = retry => retry();\nasync function run() { return retry(); }\n";
    let (ws, result) = apply_fallback(&[("caller.js", caller)]).await;
    result.expect("arrow parameter binding must not block the imported caller");
    let rewritten = ws.read("caller.js");
    assert!(
        rewritten.contains("const callback = retry => retry();"),
        "{rewritten}"
    );
    assert!(rewritten.contains("return await retry();"), "{rewritten}");
}
