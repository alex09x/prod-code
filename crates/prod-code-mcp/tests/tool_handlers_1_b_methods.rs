/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `code_make_static`, `code_convert_to_method` and `code_wrap_return` as the handlers run
//! them: how the target is named, what a dry run reports and leaves alone, and what the
//! compiler's verdict (`verify: "compile"`, a shadow run on the gateway) decides.

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::path::Path;

const UTIL_STATIC: &str = "export class Util {\n    factor: number = 2;\n\n    static helper(u: Util, x: number): number {\n        return u.factor * x;\n    }\n}\n\nexport function test() {\n    const u = new Util();\n    return Util.helper(u, 21);\n}\n";
const UTIL_METHOD: &str = "export class Util {\n    helper(x: number): number {\n        return x * 2;\n    }\n}\n\nexport function test() {\n    const u = new Util();\n    return u.helper(21);\n}\n";

fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A gateway that sees `references` at every query and whose shadow `cargo check` exits `exit`.
async fn gateway(references: Vec<Value>, exit: i64) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/references" => Value::Array(references.clone()),
        "prod-code/shadow-run" => json!({ "exit_code": exit }),
        _ => Value::Null,
    })
    .await
}

fn call_reference(file: &Path, source: &str, needle: &str) -> Value {
    let at = source.find(needle).expect("a call") + needle.find('.').map_or(0, |dot| dot + 1);
    let before = &source[..at];
    json!({
        "uri": url::Url::from_file_path(file).unwrap().to_string(),
        "range": { "start": {
            "line": before.bytes().filter(|byte| *byte == b'\n').count(),
            "character": before.rsplit('\n').next().unwrap_or_default().encode_utf16().count()
        }}
    })
}

async fn call(gw: &ScriptedGateway, root: &Path, tool: &str, args: Value) -> (bool, String) {
    let result = execute_tool(gw.addr(), root, tool, args)
        .await
        .unwrap_or_else(|err| panic!("{tool}: {err:#}"));
    (result.is_error, text_of(&result))
}

#[tokio::test]
async fn convert_to_method_reads_the_target_from_any_spelling_and_leaves_a_dry_run_alone() {
    let ws = Workspace::new(&[("util.ts", UTIL_STATIC)]);
    let root = ws.root();
    let gw = gateway(Vec::new(), 0).await;
    let (_, reference) = call(
        &gw,
        &root,
        "code_convert_to_method",
        json!({ "path": "util.ts", "method": "helper" }),
    )
    .await;
    for args in [
        json!({ "path": "util.ts", "symbol": "Util::helper" }),
        json!({ "path": "util.ts", "symbol": "Util.helper" }),
        json!({ "path": "util.ts", "symbol": "helper", "class_name": "Util" }),
        json!({ "path": "util.ts", "method_name": "helper", "struct_name": "Util" }),
        json!({ "path": "util.ts", "line": 4 }),
    ] {
        let (is_error, text) = call(&gw, &root, "code_convert_to_method", args.clone()).await;
        assert!(!is_error, "{args}: {text}");
        assert_eq!(text, reference, "{args}");
        assert_eq!(
            ws.read("util.ts"),
            UTIL_STATIC,
            "a dry run writes nothing: {args}"
        );
    }
    assert!(reference.contains("helper"), "{reference}");
}

#[tokio::test]
async fn convert_to_method_applies_after_the_compiler_accepts_and_refuses_when_it_rejects() {
    let rejected = Workspace::new(&[("util.ts", UTIL_STATIC)]);
    let gw = gateway(Vec::new(), 1).await;
    let args = json!({ "path": "util.ts", "method": "helper", "apply": true, "verify": "compile" });
    let (is_error, text) = call(
        &gw,
        &rejected.root(),
        "code_convert_to_method",
        args.clone(),
    )
    .await;
    assert!(is_error, "{text}");
    assert!(text.contains("the compiler rejects the result"), "{text}");
    assert!(
        text.contains("nothing was written: the compiler rejects it"),
        "{text}"
    );
    assert_eq!(rejected.read("util.ts"), UTIL_STATIC);

    let accepted = Workspace::new(&[("util.ts", UTIL_STATIC)]);
    let gw = gateway(Vec::new(), 0).await;
    let (is_error, text) = call(&gw, &accepted.root(), "code_convert_to_method", args).await;
    assert!(!is_error, "{text}");
    assert!(
        text.contains("the compiler accepts the result too"),
        "{text}"
    );
    let written = accepted.read("util.ts");
    assert!(written.contains("helper(x: number): number {"), "{written}");
    assert!(written.contains("return this.factor * x;"), "{written}");
    assert!(written.contains("u.helper(21)"), "{written}");
}

#[tokio::test]
async fn make_static_reads_the_target_from_any_spelling_and_the_compiler_gates_the_write() {
    let ws = Workspace::new(&[("util.ts", UTIL_METHOD)]);
    let root = ws.root();
    let refs = vec![call_reference(
        &root.join("util.ts"),
        UTIL_METHOD,
        "u.helper(21)",
    )];
    let gw = gateway(refs.clone(), 0).await;
    let (_, reference) = call(
        &gw,
        &root,
        "code_make_static",
        json!({ "path": "util.ts", "method": "helper", "class_name": "Util" }),
    )
    .await;
    for args in [
        json!({ "path": "util.ts", "method_name": "helper", "struct_name": "Util" }),
        json!({ "path": "util.ts", "line": 2, "class_name": "Util" }),
    ] {
        let (is_error, text) = call(&gw, &root, "code_make_static", args.clone()).await;
        assert!(!is_error, "{args}: {text}");
        assert_eq!(text, reference, "{args}");
        assert_eq!(
            ws.read("util.ts"),
            UTIL_METHOD,
            "a dry run writes nothing: {args}"
        );
    }

    let args = json!({ "path": "util.ts", "method": "helper", "apply": true, "verify": "compile" });
    let rejected = Workspace::new(&[("util.ts", UTIL_METHOD)]);
    let refs = vec![call_reference(
        &rejected.root().join("util.ts"),
        UTIL_METHOD,
        "u.helper(21)",
    )];
    let gw = gateway(refs, 1).await;
    let (is_error, text) = call(&gw, &rejected.root(), "code_make_static", args.clone()).await;
    assert!(is_error, "{text}");
    assert!(
        text.contains("nothing was written: the compiler rejects it"),
        "{text}"
    );
    assert_eq!(rejected.read("util.ts"), UTIL_METHOD);

    let accepted = Workspace::new(&[("util.ts", UTIL_METHOD)]);
    let refs = vec![call_reference(
        &accepted.root().join("util.ts"),
        UTIL_METHOD,
        "u.helper(21)",
    )];
    let gw = gateway(refs, 0).await;
    let (is_error, text) = call(&gw, &accepted.root(), "code_make_static", args).await;
    assert!(!is_error, "{text}");
    assert!(
        text.contains("the compiler accepts the result too"),
        "{text}"
    );
    let written = accepted.read("util.ts");
    assert!(
        written.contains("static helper(x: number): number {"),
        "{written}"
    );
    assert!(written.contains("Util.helper(21)"), "{written}");
}

const MATH: &str = "export function calculate(x: number): number {\n    return x * 2;\n}\n";
const CLIENT: &str = "import { calculate } from \"./math\";\n\nexport async function run() {\n    const a = calculate(5);\n    return a;\n}\n";

#[tokio::test]
async fn wrap_return_finds_the_declaring_file_by_symbol_and_verifies_with_the_compiler() {
    let ws = Workspace::new(&[("math.ts", MATH), ("client.ts", CLIENT)]);
    let root = ws.root();
    let refs = vec![call_reference(
        &root.join("client.ts"),
        CLIENT,
        "calculate(5)",
    )];
    let gw = gateway(refs.clone(), 0).await;
    let args =
        json!({ "symbol": "calculate", "wrapper": "promise", "verify": "compile", "apply": true });

    // the compiler rejects: nothing is written
    let rejecting = gateway(refs.clone(), 1).await;
    let (is_error, text) = call(&rejecting, &root, "code_wrap_return", args.clone()).await;
    assert!(is_error, "{text}");
    assert!(
        text.contains("nothing was written: the compiler rejects it"),
        "{text}"
    );
    assert_eq!(ws.read("math.ts"), MATH);
    assert_eq!(ws.read("client.ts"), CLIENT);

    // the compiler accepts: the declaring file is found by the symbol and both files are written
    let (is_error, text) = call(&gw, &root, "code_wrap_return", args).await;
    assert!(!is_error, "{text}");
    assert!(
        text.contains("the compiler accepts the result too"),
        "{text}"
    );
    assert!(
        ws.read("math.ts").contains("Promise<number>"),
        "{}",
        ws.read("math.ts")
    );
    assert!(
        ws.read("client.ts").contains("await calculate(5)"),
        "{}",
        ws.read("client.ts")
    );
}
