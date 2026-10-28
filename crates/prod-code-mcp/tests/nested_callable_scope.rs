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

fn rust_function_symbol(
    name: &str,
    start_line: u64,
    end_line: u64,
    name_col: u64,
    children: Vec<Value>,
) -> Value {
    json!({
        "name": name,
        "kind": 12,
        "range": {
            "start": { "line": start_line, "character": 0 },
            "end": { "line": end_line, "character": 0 }
        },
        "selectionRange": {
            "start": { "line": start_line, "character": name_col },
            "end": { "line": start_line, "character": name_col + name.len() as u64 }
        },
        "children": children
    })
}

async fn rust_gateway(symbols: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => symbols.clone(),
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
    let source = "export function outer(): boolean {\n  function inner(\n    step: number,\n  ): boolean {\n    return true;\n  }\n  function trailing(\n    step: number) {\n    return false;\n  }\n  return false;\n}\n";
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
    let trailing_start = written
        .find("function trailing(")
        .expect("trailing function");
    let trailing_end = written[trailing_start..]
        .find("\n  }")
        .map(|offset| trailing_start + offset)
        .expect("trailing body end");
    let trailing = &written[trailing_start..trailing_end];

    assert!(nested.contains("return true;"), "{written}");
    assert!(!nested.contains("!true"), "{written}");
    assert!(trailing.contains("return false;"), "{written}");
    assert!(written.contains("  return true;\n}"), "{written}");
}

#[tokio::test]
async fn rust_wrap_return_uses_line_for_a_nested_same_named_function_without_character() {
    let source = "pub struct Response<T>(pub T);\nimpl<T> Response<T> { pub fn ok(value: T) -> Self { Self(value) } }\npub fn pick() -> u32 {\n    fn pick() -> u32 { return 1; }\n    return 2;\n}\n";
    let nested = rust_function_symbol("pick", 3, 3, 7, Vec::new());
    let symbols = json!([rust_function_symbol("pick", 2, 5, 7, vec![nested])]);
    let workspace = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"nested-rust\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("selected.rs", source),
    ]);
    let gateway = rust_gateway(symbols).await;

    let report = apply(
        &gateway,
        &workspace,
        "code_wrap_return",
        json!({
            "path": "selected.rs",
            "symbol": "pick",
            "line": 4,
            "wrapper": "Response",
            "constructor": "Response::ok",
            "apply": true,
        }),
    )
    .await;
    let written = workspace.read("selected.rs");

    assert!(report.contains("pick"), "{report}");
    assert!(written.contains("pub fn pick() -> u32"), "{written}");
    assert!(written.contains("fn pick() -> Response<u32>"), "{written}");
    assert!(written.contains("return Response::ok(1);"), "{written}");
    assert!(written.contains("return 2;"), "{written}");
}

#[tokio::test]
async fn rust_wrap_return_does_not_rewrite_returns_owned_by_nested_callables() {
    let source = "pub struct Response<T>(pub T);\nimpl<T> Response<T> { pub fn ok(value: T) -> Self { Self(value) } }\npub fn outer() -> u32 {\n    fn inner(\n        step: u32,\n    ) -> u32 {\n        return 1;\n    }\n    let closure = || { return 2; };\n    return 3;\n}\n";
    let symbols = json!([rust_function_symbol("outer", 2, 6, 7, vec![])]);
    let workspace = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"nested-rust\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("selected.rs", source),
    ]);
    let gateway = rust_gateway(symbols).await;

    apply(
        &gateway,
        &workspace,
        "code_wrap_return",
        json!({
            "path": "selected.rs",
            "symbol": "outer",
            "line": 3,
            "wrapper": "Response",
            "constructor": "Response::ok",
            "apply": true,
        }),
    )
    .await;
    let written = workspace.read("selected.rs");

    assert!(written.contains("return 1;"), "{written}");
    assert!(!written.contains("Response::ok(1)"), "{written}");
    assert!(written.contains("|| { return 2; }"), "{written}");
    assert!(written.contains("return Response::ok(3);"), "{written}");
}

#[test]
fn return_ownership_skips_multiline_nested_callables_with_return_annotations() {
    for (body, selected_return) in [
        (
            "\n  function inner(\n    step: number,\n  ): number { return 1; }\n  return 2;\n",
            "return 2;",
        ),
        (
            "\n  function inner(\n    step: number) { return 1; }\n  return 2;\n",
            "return 2;",
        ),
        (
            "\n  fn inner(\n    step: i32,\n  ) -> i32 { return 1; }\n  return 2;\n",
            "return 2;",
        ),
        (
            "\n  func inner(\n    step int,\n  ) int { return 1 }\n  return 2\n",
            "return 2",
        ),
        (
            "\n  func inner(\n    step: Int\n  ) -> Int { return 1 }\n  return 2\n",
            "return 2",
        ),
    ] {
        assert_eq!(
            prod_code_mcp::invert_boolean::own_returns(body),
            vec![body.find(selected_return).unwrap()],
            "{body}"
        );
    }
}
