/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The method handlers called directly, below the symbol resolution `execute_tool` does
//! first: how their own `symbol` argument names the class and the method, and how
//! `code_wrap_return` finds the file that declares a symbol.

use super::*;
use crate::protocol::{McpContentItem, McpToolCallResult};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

const UTIL_STATIC: &str = "export class Util {\n    factor: number = 2;\n\n    static helper(u: Util, x: number): number {\n        return u.factor * x;\n    }\n}\n\nexport function test() {\n    const u = new Util();\n    return Util.helper(u, 21);\n}\n";
const UTIL_METHOD: &str = "export class Util {\n    helper(x: number): number {\n        return x * 2;\n    }\n}\n\nexport function test() {\n    const u = new Util();\n    return u.helper(21);\n}\n";

#[tokio::test]
async fn convert_method_compile_guard_rejects_non_rust_before_planning_or_writes() {
    let mut failures = Vec::new();
    for apply in [true, false] {
        for force in [false, true] {
            let ws = Workspace::new(&[
                ("util.ts", UTIL_STATIC),
                (
                    "Cargo.toml",
                    "[package]\nname = \"unrelated\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                ),
                ("src/lib.rs", "pub fn unrelated() {}\n"),
            ]);
            let commands = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let recorded = commands.clone();
            let gw = ScriptedGateway::start(move |method, params| match method {
                "textDocument/diagnostic" => answers::no_diagnostics(),
                "textDocument/references" => json!([]),
                "prod-code/shadow-run" => {
                    recorded.lock().unwrap().push(params["command"].clone());
                    json!({"exit_code": 0})
                }
                _ => Value::Null,
            })
            .await;
            let result = handle_convert_to_method(
                gw.addr(),
                &ws.root(),
                &json!({
                    "path": "util.ts", "class_name": "Util", "method": "helper",
                    "verify": "compile", "apply": apply, "force": force
                }),
            )
            .await;
            let refusal = result.err().map(|e| format!("{e:#}")).unwrap_or_default();
            let commands = commands.lock().unwrap().clone();
            if !refusal.contains("only supported for Rust")
                || !commands.is_empty()
                || ws.read("util.ts") != UTIL_STATIC
                || gw.calls() != 0
            {
                failures.push(format!(
                    "apply={apply} force={force}: refusal={refusal:?}, commands={commands:?}, source_changed={}, analyzer_calls={}",
                    ws.read("util.ts") != UTIL_STATIC, gw.calls()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn convert_method_compile_guard_keeps_conversion_without_compile_option() {
    let ws = Workspace::new(&[("util.ts", UTIL_STATIC)]);
    let gw = gateway(json!([])).await;
    let result = handle_convert_to_method(
        gw.addr(),
        &ws.root(),
        &json!({
            "path": "util.ts", "class_name": "Util", "method": "helper", "apply": true
        }),
    )
    .await
    .unwrap();
    assert!(!result.is_error, "{}", text(&result));
    assert_ne!(ws.read("util.ts"), UTIL_STATIC);
    assert!(!ws.read("util.ts").contains("static helper"));
}

fn text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect()
}

async fn gateway(refs: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/references" => refs.clone(),
        _ => Value::Null,
    })
    .await
}

fn reference(file: &Path, source: &str, needle: &str) -> Value {
    let at = source.find(needle).unwrap() + 2;
    let before = &source[..at];
    json!([{
        "uri": url::Url::from_file_path(file).unwrap().to_string(),
        "range": { "start": {
            "line": before.bytes().filter(|byte| *byte == b'\n').count(),
            "character": before.rsplit('\n').next().unwrap().len()
        }}
    }])
}

#[tokio::test]
async fn a_symbol_names_the_class_and_the_method() {
    let ws = Workspace::new(&[("util.ts", UTIL_STATIC), ("use.ts", UTIL_METHOD)]);
    let root = ws.root();
    let gw = gateway(reference(&root.join("use.ts"), UTIL_METHOD, "u.helper(21)")).await;
    let plain = text(
        &handle_make_static(
            gw.addr(),
            &root,
            &json!({ "path": "use.ts", "method": "helper", "class_name": "Util" }),
        )
        .await
        .unwrap(),
    );
    let by_method = text(
        &handle_convert_to_method(
            gw.addr(),
            &root,
            &json!({ "path": "util.ts", "method": "helper", "class_name": "Util" }),
        )
        .await
        .unwrap(),
    );
    for symbol in ["Util::helper", "Util.helper"] {
        let made = handle_make_static(
            gw.addr(),
            &root,
            &json!({ "path": "use.ts", "symbol": symbol }),
        )
        .await
        .unwrap();
        assert!(!made.is_error, "{}", text(&made));
        assert_eq!(text(&made), plain, "make_static {symbol}");
        let converted = handle_convert_to_method(
            gw.addr(),
            &root,
            &json!({ "path": "util.ts", "symbol": symbol }),
        )
        .await
        .unwrap();
        assert_eq!(text(&converted), by_method, "convert_to_method {symbol}");
    }
    // a bare name takes its class from `class_name`, or from the file when there is none
    for (tool_args, want) in [
        (
            json!({ "path": "use.ts", "symbol": "helper", "class_name": "Util" }),
            &plain,
        ),
        (json!({ "path": "use.ts", "symbol": "helper" }), &plain),
    ] {
        let made = handle_make_static(gw.addr(), &root, &tool_args)
            .await
            .unwrap();
        assert_eq!(&text(&made), want, "{tool_args}");
    }
    assert_eq!(ws.read("use.ts"), UTIL_METHOD);
    assert_eq!(ws.read("util.ts"), UTIL_STATIC);
}

#[tokio::test]
async fn wrap_return_finds_the_file_that_declares_the_symbol() {
    let math = "export function calculate(x: number): number {\n    return x * 2;\n}\n";
    let other = "export const unrelated = 1;\n";
    let ws = Workspace::new(&[("math.ts", math), ("other.ts", other)]);
    let root = ws.root();
    let gw = gateway(json!([])).await;

    let done = handle_wrap_return(
        gw.addr(),
        &root,
        &json!({ "symbol": "calculate", "wrapper": "promise", "apply": true }),
    )
    .await
    .unwrap();
    assert!(!done.is_error, "{}", text(&done));
    assert!(
        ws.read("math.ts")
            .contains("export async function calculate(x: number): Promise<number> {"),
        "{}",
        ws.read("math.ts")
    );
    assert_eq!(ws.read("other.ts"), other);

    let err = handle_wrap_return(
        gw.addr(),
        &root,
        &json!({ "symbol": "nowhere_to_be_found", "wrapper": "promise" }),
    )
    .await
    .unwrap_err();
    assert_eq!(
        format!("{err:#}"),
        "could not find file declaring symbol `nowhere_to_be_found`"
    );
}
