/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `impact` in a Rust repository with a Swift package: a changed Swift function whose callers the
//! Swift server cannot answer from a cold index is explained, not left as a bare timeout, and
//! the time the analysis may take is the caller's to set (#1960).

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

const BEFORE: &str = "struct Panel {\n    func show() {\n        print(\"one\")\n    }\n}\n";
const AFTER: &str = "struct Panel {\n    func show() {\n        print(\"two\")\n    }\n}\n";
const SWIFT: &str = "swift/Sources/Panel/Panel.swift";

fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|content| match content {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn mixed_workspace() -> Workspace {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub fn a() {}\n"),
        (
            "swift/Package.swift",
            "// swift-tools-version: 5.9\nimport PackageDescription\nlet package = Package(name: \"swift\")\n",
        ),
        (SWIFT, BEFORE),
    ]);
    ws.write(SWIFT, AFTER);
    ws
}

/// A Swift server that knows the function and answers no callers: what a server with no built
/// index of its target does.
async fn cold_swift_server(ws: &Workspace) -> ScriptedGateway {
    let uri = format!(
        "file://{}",
        std::fs::canonicalize(ws.root().join(SWIFT))
            .unwrap()
            .display()
    );
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => json!([answers::document_symbol("show", 12, 2, 4, 10)]),
        "textDocument/prepareCallHierarchy" => json!([{
            "name": "show", "kind": 12, "uri": uri,
            "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 3, "character": 5 } },
            "selectionRange": { "start": { "line": 1, "character": 9 }, "end": { "line": 1, "character": 13 } }
        }]),
        "callHierarchy/incomingCalls" => json!([]),
        _ => Value::Null,
    })
    .await
}

#[tokio::test]
async fn a_swift_function_with_no_answer_from_a_cold_index_is_explained() {
    let ws = mixed_workspace();
    let gateway = cold_swift_server(&ws).await;
    let result = execute_tool(
        gateway.addr(),
        &ws.root(),
        "code_impact",
        json!({ "depth": 1, "budget_secs": 3 }),
    )
    .await
    .expect("the analysis runs");
    let text = text_of(&result);
    assert!(
        text.contains("show"),
        "the changed function is listed: {text}"
    );
    assert!(
        text.contains("the swift language server answered no callers"),
        "the report says what the empty answer is: {text}"
    );
    assert!(text.contains("budget_secs"), "and what to do: {text}");
    assert!(text.contains("code_callers"), "{text}");
}

#[tokio::test]
async fn the_budget_is_the_callers_and_a_bad_one_is_refused() {
    let ws = mixed_workspace();
    let gateway = cold_swift_server(&ws).await;
    for bad in [0, 601] {
        let error = execute_tool(
            gateway.addr(),
            &ws.root(),
            "code_impact",
            json!({ "budget_secs": bad }),
        )
        .await
        .map(|result| text_of(&result))
        .unwrap_or_else(|error| format!("{error:#}"));
        assert!(
            error.contains("`budget_secs` is") && error.contains("between 1 and 600"),
            "{bad}: {error}"
        );
    }
}
