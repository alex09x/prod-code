/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

fn text_of(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _| match method {
        "textDocument/references" => Value::Array(Vec::new()),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
}

async fn apply(
    gateway: &ScriptedGateway,
    workspace: &Workspace,
    tool: &str,
    args: Value,
) -> String {
    let result = execute_tool(gateway.addr(), &workspace.root(), tool, args)
        .await
        .unwrap_or_else(|error| panic!("{tool}: {error:#}"));
    assert!(!result.is_error, "{}", text_of(&result));
    text_of(&result)
}

#[tokio::test]
async fn code_wrap_return_selects_the_nested_same_named_declaration_by_line() {
    let source = "export function pick(): number {\n  function pick(): number { return 2; }\n  return 1;\n}\n";
    let workspace = Workspace::new(&[
        ("selected.ts", source),
        ("package.json", "{\"type\":\"module\"}"),
    ]);
    let gateway = gateway().await;

    let report = apply(
        &gateway,
        &workspace,
        "code_wrap_return",
        json!({
            "path": "selected.ts",
            "symbol": "pick",
            "line": 2,
            "wrapper": "promise",
            "apply": true,
            "force": true,
        }),
    )
    .await;
    let written = workspace.read("selected.ts");

    assert!(report.contains("pick"), "{report}");
    assert!(
        written.starts_with("export function pick(): number {"),
        "{written}"
    );
    assert!(
        written.contains("async function pick(): Promise<number> { return 2; }"),
        "{written}"
    );
    assert!(written.contains("  return 1;\n}"), "{written}");
}

#[tokio::test]
async fn code_invert_boolean_leaves_returns_in_multiline_nested_callables_untouched() {
    let source = "export function outer(): boolean {\n  function inner(\n  ) {\n    return true;\n  }\n  return false;\n}\n";
    let workspace = Workspace::new(&[
        ("selected.ts", source),
        ("package.json", "{\"type\":\"module\"}"),
    ]);
    let gateway = gateway().await;

    apply(
        &gateway,
        &workspace,
        "code_invert_boolean",
        json!({
            "path": "selected.ts",
            "symbol": "outer",
            "new_name": "outer_inverted",
            "apply": true,
            "force": true,
        }),
    )
    .await;
    let written = workspace.read("selected.ts");
    let nested_start = written.find("function inner(").expect("nested function");
    let nested_end = written[nested_start..]
        .find("\n  }")
        .map(|offset| nested_start + offset)
        .expect("nested body end");
    let nested = &written[nested_start..nested_end];

    assert!(nested.contains("return true;"), "{written}");
    assert!(!nested.contains("!true"), "{written}");
    assert!(written.contains("  return true;\n}"), "{written}");
}
