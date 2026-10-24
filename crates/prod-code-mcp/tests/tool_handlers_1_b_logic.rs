/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `code_generify` and `code_invert_boolean` as the handlers run them: how a symbol becomes a
//! declaration, what a refusal says, how analyzer diagnostics and `force` change the verdict,
//! and what the compiler's verdict decides.

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::path::Path;

const SERVICE: &str =
    "export function processItem(item: string): void {\n    console.log(item);\n}\n";
const GENERIC: &str = "export function processItem<T extends Printable>(item: T): void {\n    console.log(item);\n}\n";
const MATH: &str = "export function isValid(x: number): boolean {\n    return x > 0;\n}\n";
const CLIENT: &str = "import { isValid } from \"./math\";\n\nexport function check() {\n    const a = isValid(5);\n    return a;\n}\n";

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

async fn call(gw: &ScriptedGateway, root: &Path, tool: &str, args: Value) -> (bool, String) {
    let result = execute_tool(gw.addr(), root, tool, args)
        .await
        .unwrap_or_else(|err| panic!("{tool}: {err:#}"));
    (result.is_error, text_of(&result))
}

async fn failure(gw: &ScriptedGateway, root: &Path, tool: &str, args: Value) -> String {
    let err = execute_tool(gw.addr(), root, tool, args.clone())
        .await
        .expect_err(&format!("{tool} {args} must fail"));
    format!("{err:#}")
}

/// Symbol hits for `workspace/symbol`; every other question gets the plain answer.
async fn symbols_gateway(hits: Vec<Value>, diagnostics: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => Value::Array(hits.clone()),
        "textDocument/diagnostic" => diagnostics.clone(),
        _ => Value::Null,
    })
    .await
}

#[tokio::test]
async fn generify_refuses_what_the_analyzer_rejects_and_force_writes_with_a_note() {
    let ws = Workspace::new(&[("service.ts", SERVICE)]);
    let root = ws.root();
    let gw = symbols_gateway(
        Vec::new(),
        answers::error_at(1, 20, "TS2304", "Cannot find name 'Printable'."),
    )
    .await;
    let args = |extra: Value| {
        let mut args = json!({ "file": "service.ts", "function": "processItem",
            "parameter": "item", "constraint": "Printable", "as": "T" });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    };
    let refusal = "the change does not compile (1 error(s)); nothing was written. The body needs more than the bound promises, or a caller no longer satisfies it or can no longer infer its type; choose another bound, fix the caller, or pass `force: true`:\n  Cannot find name 'Printable'. [TS2304] (service.ts:1:20)";
    let (is_error, preview) = call(&gw, &root, "code_generify", args(json!({}))).await;
    assert!(is_error, "{preview}");
    assert!(
        preview.contains("the analyzer rejects the result")
            && preview.contains("Cannot find name 'Printable'. [TS2304]"),
        "{preview}"
    );
    assert!(!preview.contains("[forced"), "{preview}");
    assert_eq!(
        failure(&gw, &root, "code_generify", args(json!({ "apply": true }))).await,
        refusal
    );
    assert_eq!(ws.read("service.ts"), SERVICE, "diagnostics stop the write");

    let (is_error, text) = call(
        &gw,
        &root,
        "code_generify",
        args(json!({ "apply": true, "force": true })),
    )
    .await;
    assert!(!is_error, "{text}");
    assert!(
        text.ends_with("\n[forced: applied with diagnostics]"),
        "{text}"
    );
    assert_eq!(ws.read("service.ts"), GENERIC);
}

async fn invert_gateway(refs: Value, exit: i64) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/references" => refs.clone(),
        "prod-code/shadow-run" => json!({ "exit_code": exit }),
        _ => Value::Null,
    })
    .await
}

fn client_reference(client: &Path) -> Value {
    let at = CLIENT.find("isValid(5)").unwrap();
    let before = &CLIENT[..at];
    json!([{
        "uri": url::Url::from_file_path(client).unwrap().to_string(),
        "range": { "start": {
            "line": before.bytes().filter(|byte| *byte == b'\n').count(),
            "character": before.rsplit('\n').next().unwrap().len()
        }}
    }])
}

#[tokio::test]
async fn invert_boolean_previews_then_writes_only_what_the_compiler_accepts() {
    let make = || Workspace::new(&[("math.ts", MATH), ("client.ts", CLIENT)]);
    let args = |extra: Value| {
        let mut args = json!({ "path": "math.ts", "symbol": "isValid", "new_name": "isInvalid" });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    };

    let ws = make();
    let root = ws.root();
    let gw = invert_gateway(client_reference(&root.join("client.ts")), 0).await;
    let (is_error, preview) = call(&gw, &root, "code_invert_boolean", args(json!({}))).await;
    assert!(!is_error, "{preview}");
    assert!(preview.contains("isInvalid"), "{preview}");
    assert_eq!(ws.read("math.ts"), MATH, "a dry run writes nothing");
    assert_eq!(ws.read("client.ts"), CLIENT);

    let rejected = make();
    let gw = invert_gateway(client_reference(&rejected.root().join("client.ts")), 1).await;
    let verify = args(json!({ "apply": true, "verify": "compile" }));
    let (is_error, text) = call(&gw, &rejected.root(), "code_invert_boolean", verify.clone()).await;
    assert!(is_error, "{text}");
    assert!(
        text.contains("nothing was written: the compiler rejects it"),
        "{text}"
    );
    assert_eq!(rejected.read("math.ts"), MATH);
    assert_eq!(rejected.read("client.ts"), CLIENT);

    let accepted = make();
    let gw = invert_gateway(client_reference(&accepted.root().join("client.ts")), 0).await;
    let (is_error, text) = call(&gw, &accepted.root(), "code_invert_boolean", verify).await;
    assert!(!is_error, "{text}");
    assert!(
        text.contains("the compiler accepts the result too"),
        "{text}"
    );
    assert!(
        accepted
            .read("math.ts")
            .contains("export function isInvalid(x: number): boolean {")
    );
    assert!(accepted.read("math.ts").contains("return !(x > 0);"));
    assert!(accepted.read("client.ts").contains("!isInvalid(5)"));
}

#[tokio::test]
async fn invert_boolean_refuses_to_write_past_a_reference_it_cannot_rewrite() {
    let client = "import { isValid as check } from './math';\nexport function run() {\n    return check(5);\n}\n";
    let ws = Workspace::new(&[("math.ts", MATH), ("client.ts", client)]);
    let root = ws.root();
    let at = client.find("check(5)").unwrap();
    let before = &client[..at];
    let refs = json!([{
        "uri": url::Url::from_file_path(root.join("client.ts")).unwrap().to_string(),
        "range": { "start": {
            "line": before.bytes().filter(|byte| *byte == b'\n').count(),
            "character": before.rsplit('\n').next().unwrap().len()
        }}
    }]);
    let gw = invert_gateway(refs, 0).await;
    let message = failure(
        &gw,
        &root,
        "code_invert_boolean",
        json!({ "path": "math.ts", "symbol": "isValid", "new_name": "isInvalid", "apply": true }),
    )
    .await;
    assert_eq!(
        message,
        "2 reference(s) to `isValid` were not negated and would mean the opposite; nothing was written:\n  client.ts:1:10 imports `isValid` through an alias; alias call sites are not resolved\n  client.ts:3:12: analyzer reference to `isValid` could not be inverted safely"
    );
    assert_eq!(ws.read("math.ts"), MATH);
    assert_eq!(ws.read("client.ts"), client);
}
