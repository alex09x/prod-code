/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `code_codemod` and `code_schema_rename` on the paths the happy-path suites leave: the
//! analyzer's structural replace written to disk or failing, and a rename the analyzer rejects.

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::path::Path;

const OLD: &str = "fn a(x: Option<i32>) -> i32 {\n    x.unwrap()\n}\n";
const NEW: &str = "fn a(x: Option<i32>) -> i32 {\n    x.expect(\"invariant\")\n}\n";
const BEFORE: &str = "pub struct Order {\n    pub order_id: String,\n}\n";
const AFTER: &str = "pub struct Order {\n    pub trade_id: String,\n}\n";
const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

async fn try_call(
    gw: &ScriptedGateway,
    root: &Path,
    tool: &str,
    args: Value,
) -> anyhow::Result<(bool, String)> {
    let result = execute_tool(gw.addr(), root, tool, args).await?;
    let text = result
        .content
        .iter()
        .map(|McpContentItem::Text { text }| text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    Ok((result.is_error, text))
}

async fn call(gw: &ScriptedGateway, root: &Path, tool: &str, args: Value) -> (bool, String) {
    try_call(gw, root, tool, args)
        .await
        .unwrap_or_else(|err| panic!("{tool}: {err:#}"))
}

fn order_workspace() -> Workspace {
    Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", BEFORE)])
}

/// Renames `order_id` in the one file of each root the analyzer is asked about, and answers
/// every diagnostics request with `diagnostics`.
async fn rename_gateway(roots: &[&Workspace], diagnostics: Value) -> ScriptedGateway {
    let plans: Vec<(Value, Value)> = roots
        .iter()
        .map(|ws| {
            let path = ws.root().join("src/lib.rs");
            (
                json!(answers::uri(&path)),
                answers::whole_file(&path, BEFORE, AFTER),
            )
        })
        .collect();
    ScriptedGateway::start(move |method, params| match method {
        "textDocument/rename" => plans
            .iter()
            .find(|(uri, _)| *uri == params["textDocument"]["uri"])
            .map_or(Value::Null, |(_, edit)| edit.clone()),
        "textDocument/diagnostic" => diagnostics.clone(),
        _ => Value::Null,
    })
    .await
}

#[tokio::test]
async fn the_analyzers_structural_replace_is_written_inside_the_scope_asked_for() {
    let ws = Workspace::new(&[("src/lib.rs", OLD), ("src/other.rs", OLD)]);
    let root = ws.root();
    let answer = answers::whole_file(&root.join("src/lib.rs"), OLD, NEW);
    let gw = ScriptedGateway::start(move |method, _| match method {
        "prodCode/structuralReplace" => answer.clone(),
        _ => Value::Null,
    })
    .await;
    // No syntax tree here matches this rule, so the analyzer's answer is what is applied.
    let rule = "missing($a) ==>> present($a)";
    let args = json!({ "rule": rule, "path": "src/lib.rs", "apply": true });
    let (is_error, text) = call(&gw, &ws.root(), "code_codemod", args).await;

    assert!(!is_error, "{text}");
    assert!(
        text.starts_with(&format!("`{rule}`\n2 changed line(s) in 1 file(s)\n\n")),
        "{text}"
    );
    assert!(text.contains("a/src/lib.rs"), "{text}");
    assert!(
        text.ends_with("[applied to 1 file(s): src/lib.rs]"),
        "{text}"
    );
    assert_eq!(ws.read("src/lib.rs"), NEW);
    assert_eq!(ws.read("src/other.rs"), OLD);
}

#[tokio::test]
async fn an_analyzer_that_fails_the_replace_leaves_it_matching_nothing() {
    let ws = Workspace::new(&[("src/lib.rs", OLD)]);
    let gw = ScriptedGateway::start(|method, _| match method {
        "prodCode/structuralReplace" => answers::failure("the analyzer is not ready"),
        _ => Value::Null,
    })
    .await;
    // Nothing in the checkout matches, so the analyzer is asked; it fails.
    let rule = "missing($a) ==>> present($a)";
    let (is_error, text) = call(&gw, &ws.root(), "code_codemod", json!({ "rule": rule })).await;
    assert!(!is_error);
    assert_eq!(text, format!("`{rule}` matches nothing in this workspace"));
    assert_eq!(ws.read("src/lib.rs"), OLD);
}

#[tokio::test]
async fn a_rename_the_analyzer_rejects_is_refused_unless_forced() {
    let ws = order_workspace();
    let rejected = answers::error_at(2, 9, "E0412", "cannot find type `Trade` in this scope");
    let gw = rename_gateway(&[&ws], rejected).await;
    let args = |extra: Value| {
        let mut args = json!({ "field": "order_id", "to": "trade_id", "repos": null });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    };

    let err = try_call(
        &gw,
        &ws.root(),
        "code_schema_rename",
        args(json!({ "apply": true })),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "the rename does not compile (1 error(s)); nothing was written. Pass `force: true` to write it anyway:\n  cannot find type `Trade` in this scope [E0412] (src/lib.rs:2:9)"
    );
    assert_eq!(ws.read("src/lib.rs"), BEFORE);

    // A preview shows the rejection as an error result, and writes nothing either.
    let (is_error, text) = call(&gw, &ws.root(), "code_schema_rename", args(json!({}))).await;
    assert!(is_error, "{text}");
    assert!(
        text.contains("cannot find type `Trade` in this scope"),
        "{text}"
    );
    assert!(!text.contains("[applied"), "{text}");
    assert_eq!(ws.read("src/lib.rs"), BEFORE);

    let (is_error, json_text) = call(
        &gw,
        &ws.root(),
        "code_schema_rename",
        args(json!({ "workspace_edit": true })),
    )
    .await;
    assert!(is_error, "{json_text}");
    let edit: Value = serde_json::from_str(&json_text).expect("a workspace edit");
    assert_eq!(edit["documentChanges"][0]["edits"][0]["newText"], AFTER);

    let forced = json!({ "apply": true, "force": true, "path": "src" });
    let (is_error, text) = call(&gw, &ws.root(), "code_schema_rename", args(forced)).await;
    assert!(
        is_error,
        "an applied rename the analyzer rejects is still reported: {text}"
    );
    assert!(text.contains("[applied to 1 file(s)]"), "{text}");
    assert_eq!(ws.read("src/lib.rs"), AFTER);
}

#[tokio::test]
async fn a_rename_across_repositories_reports_the_analyzers_rejection_as_an_error() {
    let a = order_workspace();
    let b = order_workspace();
    let rejected = answers::error_at(2, 9, "E0412", "cannot find type `Trade` in this scope");
    let gw = rename_gateway(&[&a, &b], rejected).await;
    let repos = json!([b.root().display().to_string()]);
    let args = |extra: Value| {
        let mut args = json!({ "field": "order_id", "to": "trade_id", "repos": repos });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    };

    let (is_error, text) = call(&gw, &a.root(), "code_schema_rename", args(json!({}))).await;
    assert!(is_error, "{text}");
    assert!(text.contains("`order_id` → `trade_id`"), "{text}");
    assert!(
        text.contains("cannot find type `Trade` in this scope"),
        "{text}"
    );
    assert!(!text.contains("[applied"), "{text}");

    let (is_error, json_text) = call(
        &gw,
        &a.root(),
        "code_schema_rename",
        args(json!({ "workspace_edit": true })),
    )
    .await;
    assert!(is_error, "{json_text}");
    let edit: Value = serde_json::from_str(&json_text).expect("a workspace edit");
    assert_eq!(edit["documentChanges"].as_array().map(Vec::len), Some(2));
    assert_eq!(a.read("src/lib.rs"), BEFORE);
    assert_eq!(b.read("src/lib.rs"), BEFORE);
}
