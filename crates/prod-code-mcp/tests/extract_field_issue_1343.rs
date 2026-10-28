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
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::process::Command;

async fn gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

fn result_text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn public_extract_field_keeps_regex_literal_text_unchanged() {
    let source = r#"const quotedPattern = /"42/.test('\"42');
const commentPattern = /[/*]42/.test("/42");
export class Calc {
  value() {
    const matched = /42/.test("42");
    let numerator = 84;
    const quotient = numerator++ / 42;
    return matched && quotedPattern && commentPattern && quotient === 2 ? 42 : 0;
  }
}

if (new Calc().value() !== 42) throw new Error("regex changed");
"#;
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
        ),
        ("package.json", "{\"type\":\"module\"}\n"),
        ("calc.js", source),
    ]);
    let remote = gateway().await;
    let result = prod_code_mcp::tools::execute_tool(
        remote.addr(),
        &ws.root(),
        "code_extract_field",
        serde_json::json!({
            "path": "calc.js",
            "range": "8:75-8:77",
            "name": "magic",
            "type": "number",
            "replace_all": true,
            "apply": true
        }),
    )
    .await
    .expect("public extract-field call");
    assert!(!result.is_error, "{}", result_text(&result));
    let updated = ws.read("calc.js");
    assert!(updated.contains("/42/.test(\"42\")"), "{updated}");
    assert!(updated.contains("/\"42/.test"), "{updated}");
    assert!(updated.contains("/[/*]42/.test"), "{updated}");
    assert!(!updated.contains("/this.magic/"), "{updated}");
    assert!(updated.contains("numerator++ / this.magic"), "{updated}");
    assert!(
        updated.contains(
            "return matched && quotedPattern && commentPattern && quotient === 2 ? this.magic : 0;"
        ),
        "{updated}"
    );

    let output = Command::new("node")
        .arg(ws.path("calc.js"))
        .output()
        .expect("run JavaScript runtime control");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn public_extract_field_uses_the_selected_instance_method_scope() {
    let source = r#"export class Counter {
  static initial = 0;
  static step = 1;
  static current = `first
static`

  value() {
    return 42;
  }
}

if (Counter.initial !== 0 || Counter.step !== 1 || Counter.current !== `first
static` || new Counter().value() !== 42) {
  throw new Error("static/instance field scope changed");
}
"#;
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
        ),
        ("package.json", "{\"type\":\"module\"}\n"),
        ("counter.js", source),
    ]);
    let remote = gateway().await;
    let result = prod_code_mcp::tools::execute_tool(
        remote.addr(),
        &ws.root(),
        "code_extract_field",
        serde_json::json!({
            "path": "counter.js",
            "line": 8,
            "character": 12,
            "end_line": 8,
            "end_character": 14,
            "name": "magic",
            "type": "number",
            "apply": true
        }),
    )
    .await
    .expect("public extract-field call");
    assert!(!result.is_error, "{}", result_text(&result));
    let updated = ws.read("counter.js");
    assert!(updated.contains("    magic = 42;"), "{updated}");
    assert!(!updated.contains("static magic = 42;"), "{updated}");
    assert!(updated.contains("return this.magic;"), "{updated}");

    let output = Command::new("node")
        .arg(ws.path("counter.js"))
        .output()
        .expect("run JavaScript runtime control");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn public_extract_field_handles_typescript_object_types_in_method_signatures() {
    let source = r#"export class Counter {
  value(options: { flag: boolean }): number {
    return options.flag ? 42 : 0;
  }
}
"#;
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
        ),
        ("package.json", "{\"type\":\"module\"}\n"),
        ("counter.ts", source),
    ]);
    let remote = gateway().await;
    let result = prod_code_mcp::tools::execute_tool(
        remote.addr(),
        &ws.root(),
        "code_extract_field",
        serde_json::json!({
            "path": "counter.ts",
            "range": "3:27-3:29",
            "name": "magic",
            "type": "number",
            "apply": true
        }),
    )
    .await
    .expect("public TypeScript extract-field call");
    assert!(!result.is_error, "{}", result_text(&result));
    let updated = ws.read("counter.ts");
    assert!(updated.contains("magic: number = 42;"), "{updated}");
    assert!(
        updated.contains("return options.flag ? this.magic : 0;"),
        "{updated}"
    );
}

#[tokio::test]
async fn public_extract_field_keeps_code_after_regex_comment_starters() {
    let source = r#"const commentPattern = /[/*]42/.test("/42");
export class Calc {
  value() {
    const quotient = 84 / 42;
    return commentPattern && quotient === 2 ? 42 : 0;
  }
}

if (new Calc().value() !== 42) throw new Error("regex comment starter changed");
"#;
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
        ),
        ("package.json", "{\"type\":\"module\"}\n"),
        ("calc.js", source),
    ]);
    let remote = gateway().await;
    let result = prod_code_mcp::tools::execute_tool(
        remote.addr(),
        &ws.root(),
        "code_extract_field",
        serde_json::json!({
            "path": "calc.js",
            "range": "5:47-5:49",
            "name": "magic",
            "type": "number",
            "replace_all": true,
            "apply": true
        }),
    )
    .await
    .expect("public extract-field call");
    assert!(!result.is_error, "{}", result_text(&result));
    let updated = ws.read("calc.js");
    assert!(updated.contains("/[/*]42/.test(\"/42\")"), "{updated}");
    assert!(updated.contains("84 / this.magic"), "{updated}");
    assert!(
        updated.contains("return commentPattern && quotient === 2 ? this.magic : 0;"),
        "{updated}"
    );
    let output = Command::new("node")
        .arg(ws.path("calc.js"))
        .output()
        .expect("run JavaScript runtime control");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
