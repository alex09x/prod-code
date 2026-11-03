/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::json;

const MISSING_HTML: &str = "the HTML language server did not start: Failed to execute command: vscode-html-language-server [--stdio]: No such file or directory (os error 2) (no fallback: no managed backend language server for engine `html`)";

async fn lookup(
    source: &str,
    name: &str,
) -> anyhow::Result<prod_code_mcp::protocol::McpToolCallResult> {
    let ws = Workspace::new(&[("sample.html", source)]);
    let node = ScriptedGateway::start(|method, _| {
        if method == "prod-code/handshake" {
            json!({"refuse_handshake":MISSING_HTML})
        } else {
            serde_json::Value::Null
        }
    })
    .await;
    execute_tool(
        node.addr(),
        &ws.root(),
        "code_definition",
        json!({
            "path":"sample.html","symbol":name,"budget_ms":1500
        }),
    )
    .await
}

#[tokio::test]
async fn issue_1846_missing_html_backend_returns_a_bounded_local_function() {
    let result = lookup(
        include_str!("../../../fixtures/issue-1846-inline.html"),
        "sortTable",
    )
    .await
    .unwrap();
    assert!(!result.is_error, "{result:?}");
    let text = serde_json::to_string(&result).unwrap();
    assert!(text.contains("syntax-only"), "{text}");
    assert!(text.contains("return index + 1"), "{text}");
    assert_eq!(
        result.structured_content.as_ref().unwrap()["semantic"],
        false
    );
}

#[tokio::test]
async fn issue_1846_html_comment_and_string_decoys_do_not_become_definitions() {
    let source = "<!-- <script>function ghost() {}</script> -->\n<script>\nconst text = 'function ghost() {}';\nfunction real() { return /}/.test(text); }\n</script>\n";
    let real = lookup(source, "real").await.unwrap();
    assert!(!real.is_error, "{real:?}");
    let ghost = lookup(source, "ghost").await;
    assert!(
        ghost.is_err() || ghost.as_ref().is_ok_and(|reply| reply.is_error),
        "{ghost:?}"
    );
}

#[tokio::test]
async fn issue_1846_duplicate_local_html_functions_are_reported_as_ambiguous() {
    let result=lookup("<script>function sortTable() { return 1; }</script>\n<script>function sortTable() { return 2; }</script>\n","sortTable").await.unwrap();
    assert!(result.is_error, "{result:?}");
    assert!(
        serde_json::to_string(&result)
            .unwrap()
            .contains("ambiguous")
    );
    let same_line = lookup(
        "<script>function sortTable() { return 1; } function sortTable() { return 2; }</script>",
        "sortTable",
    )
    .await
    .unwrap();
    assert!(
        same_line.is_error,
        "minified duplicate must not pick the first: {same_line:?}"
    );
}

#[tokio::test]
async fn issue_1846_html_fallback_uses_utf16_columns_and_bounds_the_body() {
    let source =
        "<script>const icon = '😀'; function sortTable() { return /}/.test(icon); }</script>\n";
    let result = lookup(source, "sortTable").await.unwrap();
    assert!(!result.is_error, "{result:?}");
    let column = source[..source.find("sortTable").unwrap()]
        .encode_utf16()
        .count()
        + 1;
    assert_eq!(
        result.structured_content.as_ref().unwrap()["locations"][0]["character"],
        column
    );
    let long = format!(
        "<script>\nfunction sortTable() {{\n{}return 1;\n}}\n</script>\n",
        "void 0;\n".repeat(400)
    );
    let bounded = lookup(&long, "sortTable").await.unwrap();
    assert_eq!(
        bounded.structured_content.as_ref().unwrap()["truncated"],
        true
    );
    assert!(serde_json::to_string(&bounded).unwrap().len() < 30 * 1024);
}

#[tokio::test]
async fn issue_1846_html_fallback_preserves_security_failures_and_external_confinement() {
    let ws = Workspace::new(&[(
        "sample.html",
        "<script>function sortTable() { return 1; }</script>",
    )]);
    let node = ScriptedGateway::start(|method, _| {
        if method == "prod-code/handshake" {
            json!({"refuse_handshake":format!("TLS certificate rejected; {MISSING_HTML}")})
        } else {
            serde_json::Value::Null
        }
    })
    .await;
    let result = execute_tool(
        node.addr(),
        &ws.root(),
        "code_definition",
        json!({"path":"sample.html","symbol":"sortTable"}),
    )
    .await;
    assert!(
        result.is_err(),
        "security failure must stay an error: {result:?}"
    );
    let outside = Workspace::new(&[(
        "sample.html",
        "<script>function secret() { return 123; }</script>",
    )]);
    let missing = ScriptedGateway::start(|method, _| {
        if method == "prod-code/handshake" {
            json!({"refuse_handshake":MISSING_HTML})
        } else {
            serde_json::Value::Null
        }
    })
    .await;
    let result = execute_tool(
        missing.addr(),
        &ws.root(),
        "code_definition",
        json!({"path":outside.root().join("sample.html"),"symbol":"secret"}),
    )
    .await;
    assert!(
        result.is_err() || result.as_ref().is_ok_and(|r| r.is_error),
        "{result:?}"
    );
}
