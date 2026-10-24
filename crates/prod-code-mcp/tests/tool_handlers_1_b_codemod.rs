/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `code_codemod` and `code_schema_rename` as the handlers run them: the local syntax-tree
//! engine, the analyzer's structural replace behind it, the 6000-character diff cap, and what the
//! compiler's verdict decides for a schema rename.

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::path::Path;

const RULE: &str = "console.log($A) ==>> logger.info($A)";
const LOG_TS: &str = "console.log(1);\nconsole.log(2);\n";

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

async fn silent() -> ScriptedGateway {
    ScriptedGateway::start(|_, _| Value::Null).await
}

#[tokio::test]
async fn the_syntax_tree_engine_reports_a_diff_and_writes_only_when_asked() {
    let ws = Workspace::new(&[("a.ts", LOG_TS), ("b.ts", "const x = 1;\n")]);
    let root = ws.root();
    let gw = silent().await;
    let (is_error, dry) = call(&gw, &root, "code_codemod", json!({ "rule": RULE })).await;
    assert!(!is_error, "{dry}");
    assert!(dry.starts_with(&format!("`{RULE}`\n")), "{dry}");
    assert!(dry.contains("changed line(s) in 1 file(s)"), "{dry}");
    assert!(
        dry.contains("-console.log(1);") && dry.contains("+logger.info(1);"),
        "{dry}"
    );
    assert!(
        dry.ends_with("nothing was written; pass `apply: true` to make these edits"),
        "{dry}"
    );
    assert_eq!(ws.read("a.ts"), LOG_TS, "a dry run writes nothing");

    let (is_error, applied) = call(
        &gw,
        &root,
        "code_codemod",
        json!({ "rule": RULE, "path": "a.ts", "apply": true }),
    )
    .await;
    assert!(!is_error, "{applied}");
    assert!(
        applied.ends_with("[applied to 1 file(s): a.ts]"),
        "{applied}"
    );
    assert_eq!(ws.read("a.ts"), "logger.info(1);\nlogger.info(2);\n");
    assert_eq!(ws.read("b.ts"), "const x = 1;\n");
}

#[tokio::test]
async fn a_diff_over_the_cap_is_cut_and_says_so() {
    let many: String = (0..400).map(|i| format!("console.log({i});\n")).collect();
    let ws = Workspace::new(&[("many.ts", &many)]);
    let gw = silent().await;
    let (is_error, text) = call(&gw, &ws.root(), "code_codemod", json!({ "rule": RULE })).await;
    assert!(!is_error);
    assert!(text.contains("\n… diff truncated\n"), "{text}");
    assert!(text.contains("changed line(s) in 1 file(s)"), "{text}");
    assert!(
        text.ends_with("nothing was written; pass `apply: true` to make these edits"),
        "{text}"
    );
    assert!(text.len() < 6300, "{} bytes", text.len());
}

#[tokio::test]
async fn nothing_matching_is_reported_with_its_scope() {
    let ws = Workspace::new(&[("a.py", "x = 1\n"), ("lib.rs", "pub fn f() {}\n")]);
    let root = ws.root();
    let gw = silent().await;
    let rule = "$a.ok() ==>> $a.ok_or(())";
    // a Python scope is not asked of the Rust analyzer; the whole workspace is, and it has no answer
    let (_, scoped) = call(
        &gw,
        &root,
        "code_codemod",
        json!({ "rule": rule, "path": "a.py" }),
    )
    .await;
    assert_eq!(
        scoped,
        format!(
            "`{rule}` matches nothing in {}",
            root.join("a.py").display()
        )
    );
    let (_, whole) = call(&gw, &root, "code_codemod", json!({ "rule": rule })).await;
    assert_eq!(whole, format!("`{rule}` matches nothing in this workspace"));
    assert_eq!(
        gw.calls(),
        1,
        "only the Rust-capable scope asks the analyzer"
    );
}

#[tokio::test]
async fn the_analyzers_structural_replace_is_previewed_cut_and_applied() {
    let old = "fn a(x: Option<i32>) -> i32 {\n    x.unwrap()\n}\n";
    let new = "fn a(x: Option<i32>) -> i32 {\n    x.expect(\"invariant\")\n}\n";
    let rule = "$a.unwrap() ==>> $a.expect(\"invariant\")";
    let ws = Workspace::new(&[("src/lib.rs", old)]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");
    let answer = answers::whole_file(&lib, old, new);
    let gw = ScriptedGateway::start(move |method, _| match method {
        "prodCode/structuralReplace" => answer.clone(),
        _ => Value::Null,
    })
    .await;
    let (is_error, dry) = call(&gw, &root, "code_codemod", json!({ "rule": rule })).await;
    assert!(!is_error, "{dry}");
    assert!(
        dry.contains("-    x.unwrap()") && dry.contains("+    x.expect(\"invariant\")"),
        "{dry}"
    );
    assert!(dry.contains("a/src/lib.rs"), "{dry}");
    assert!(
        dry.ends_with("nothing was written; pass `apply: true` to make these edits"),
        "{dry}"
    );
    assert_eq!(ws.read("src/lib.rs"), old);

    // a rewrite longer than the cap is cut
    let long_old: String = (0..400)
        .map(|i| format!("fn f{i}() {{ let _ = 1; }}\n"))
        .collect();
    let long_new = long_old.replace("let _ = 1;", "let _ = 2;");
    let ws = Workspace::new(&[("src/lib.rs", &long_old)]);
    let root = ws.root();
    let answer = answers::whole_file(&root.join("src/lib.rs"), &long_old, &long_new);
    let gw = ScriptedGateway::start(move |method, _| match method {
        "prodCode/structuralReplace" => answer.clone(),
        _ => Value::Null,
    })
    .await;
    let (_, cut) = call(
        &gw,
        &root,
        "code_codemod",
        json!({ "rule": "let _ = 1; ==>> let _ = 2;" }),
    )
    .await;
    assert!(cut.contains("\n… diff truncated\n"), "{cut}");
    assert!(cut.contains("800 changed line(s) in 1 file(s)"), "{cut}");
    assert_eq!(ws.read("src/lib.rs"), long_old);
}

fn order_workspace() -> (Workspace, &'static str, &'static str) {
    const BEFORE: &str = "pub struct Order {\n    pub order_id: String,\n}\n";
    const AFTER: &str = "pub struct Order {\n    pub trade_id: String,\n}\n";
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", BEFORE),
    ]);
    (ws, BEFORE, AFTER)
}

async fn rename_gateway(root: &Path, before: &str, after: &str, exit: i64) -> ScriptedGateway {
    let rename = answers::whole_file(&root.join("src/lib.rs"), before, after);
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/rename" => rename.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "prod-code/shadow-run" => json!({ "exit_code": exit }),
        _ => Value::Null,
    })
    .await
}

#[tokio::test]
async fn a_schema_rename_is_written_only_when_the_compiler_accepts_it() {
    let args = json!({ "field": "order_id", "to": "trade_id", "apply": true, "verify": "compile" });
    let (rejected, before, after) = order_workspace();
    let gw = rename_gateway(&rejected.root(), before, after, 1).await;
    let (is_error, text) = call(&gw, &rejected.root(), "code_schema_rename", args.clone()).await;
    assert!(is_error, "{text}");
    assert!(
        text.contains("nothing was written: the compiler rejects it"),
        "{text}"
    );
    assert_eq!(rejected.read("src/lib.rs"), before);

    let (accepted, before, after) = order_workspace();
    let gw = rename_gateway(&accepted.root(), before, after, 0).await;
    let (is_error, text) = call(&gw, &accepted.root(), "code_schema_rename", args).await;
    assert!(!is_error, "{text}");
    assert!(
        text.contains("the compiler accepts the result too"),
        "{text}"
    );
    assert_eq!(accepted.read("src/lib.rs"), after);
}

#[tokio::test]
async fn a_schema_rename_workspace_edit_reports_the_compilers_verdict_by_error_status() {
    let (ws, before, after) = order_workspace();
    let args = json!({ "field": "order_id", "to": "trade_id", "workspace_edit": true, "verify": "compile" });
    let gw = rename_gateway(&ws.root(), before, after, 0).await;
    let (is_error, text) = call(&gw, &ws.root(), "code_schema_rename", args.clone()).await;
    assert!(!is_error, "{text}");
    let edit: Value = serde_json::from_str(&text).expect("a workspace edit");
    assert_eq!(edit["documentChanges"][0]["edits"][0]["newText"], after);

    let gw = rename_gateway(&ws.root(), before, after, 1).await;
    let (is_error, text) = call(&gw, &ws.root(), "code_schema_rename", args).await;
    assert!(is_error, "a rejected rename is an error result: {text}");
    assert_eq!(ws.read("src/lib.rs"), before);
}

#[tokio::test]
async fn a_schema_rename_across_repositories_lands_in_all_of_them_or_none() {
    let (a, before, after) = order_workspace();
    let (b, _, _) = order_workspace();
    let c = Workspace::new(&[("src/lib.rs", "pub struct Other;\n")]);
    let plans: Vec<(Value, Value)> = [&a, &b]
        .iter()
        .map(|ws| {
            let path = ws.root().join("src/lib.rs");
            (
                json!(answers::uri(&path)),
                answers::whole_file(&path, before, after),
            )
        })
        .collect();
    let gw = ScriptedGateway::start(move |method, params| match method {
        "textDocument/rename" => plans
            .iter()
            .find(|(uri, _)| *uri == params["textDocument"]["uri"])
            .map_or(Value::Null, |(_, edit)| edit.clone()),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let repos = json!([
        b.root().display().to_string(),
        c.root().display().to_string()
    ]);
    let args = |extra: Value| {
        let mut args = json!({ "field": "order_id", "to": "trade_id", "repos": repos });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    };

    let (is_error, dry) = call(&gw, &a.root(), "code_schema_rename", args(json!({}))).await;
    assert!(!is_error, "{dry}");
    assert!(dry.contains("`order_id` → `trade_id`"), "{dry}");
    assert!(!dry.contains("[applied"), "{dry}");
    assert_eq!(a.read("src/lib.rs"), before);
    assert_eq!(b.read("src/lib.rs"), before);

    let (is_error, json_text) = call(
        &gw,
        &a.root(),
        "code_schema_rename",
        args(json!({ "workspace_edit": true })),
    )
    .await;
    assert!(!is_error, "{json_text}");
    let edit: Value = serde_json::from_str(&json_text).expect("a workspace edit");
    assert_eq!(
        edit["documentChanges"].as_array().map(Vec::len),
        Some(2),
        "{json_text}"
    );

    let (is_error, applied) = call(
        &gw,
        &a.root(),
        "code_schema_rename",
        args(json!({ "apply": true })),
    )
    .await;
    assert!(!is_error, "{applied}");
    assert_eq!(a.read("src/lib.rs"), after);
    assert_eq!(b.read("src/lib.rs"), after);
    assert_eq!(c.read("src/lib.rs"), "pub struct Other;\n");
}
