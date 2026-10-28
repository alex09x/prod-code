/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

const LIB: &str = "#[path = \"shared.rs\"]\npub mod shared;\n\npub fn calculate() -> i32 {\n    let value = 2 + 3;\n    value\n}\n";
const EXTRACTED: &str = "#[path = \"shared.rs\"]\npub mod shared;\n\npub fn calculate() -> i32 {\n    let value = fun_name();\n    value\n}\n\nfn fun_name() -> i32 {\n    let value = 2 + 3;\n    value\n}\n";
const SHARED: &str = "pub fn same_target() -> i32 {\n    let value = 2 + 3;\n    value\n}\n";
const BIN: &str = "fn main() {\n    let value = 2 + 3;\n    println!(\"{value}\");\n}\n";
const SHARED_BIN: &str = "#[path = \"../shared.rs\"]\nmod shared;\nfn main() {}\n";
const MANIFEST: &str = "[package]\nname = \"target-boundary\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n\n[[bin]]\nname = \"runner\"\npath = \"src/bin/runner.rs\"\n";

fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn gateway(workspace: &Workspace) -> ScriptedGateway {
    let selected = workspace.root().join("src/lib.rs");
    ScriptedGateway::start(move |method, _| match method {
        "prodCode/applyAssist" => answers::whole_file(&selected, LIB, EXTRACTED),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "prod-code/shadow-run" => json!({ "exit_code": 0 }),
        _ => Value::Null,
    })
    .await
}

#[tokio::test]
async fn other_file_duplicates_stay_within_the_selected_cargo_target() {
    let workspace = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", LIB),
        ("src/shared.rs", SHARED),
        ("src/bin/runner.rs", BIN),
    ]);
    let gateway = gateway(&workspace).await;

    let result = execute_tool(
        gateway.addr(),
        &workspace.root(),
        "code_extract_function",
        json!({
            "path": "src/lib.rs",
            "line": 5,
            "character": 5,
            "end_line": 5,
            "end_character": 23,
            "name": "calculate_value",
            "duplicates": true,
            "other_files": true,
            "apply": true,
        }),
    )
    .await
    .expect("the public extraction operation should return a plan");
    let report = text_of(&result);

    assert!(!result.is_error, "{report}");
    assert!(
        workspace
            .read("src/shared.rs")
            .contains("crate::calculate_value()"),
        "report={report}\nshared={}\nbin={}",
        workspace.read("src/shared.rs"),
        workspace.read("src/bin/runner.rs")
    );
    assert_eq!(workspace.read("src/bin/runner.rs"), BIN);
}

#[tokio::test]
async fn a_path_attributed_module_shared_by_targets_is_not_a_duplicate() {
    let workspace = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", LIB),
        ("src/shared.rs", SHARED),
        ("src/bin/runner.rs", SHARED_BIN),
    ]);
    let gateway = gateway(&workspace).await;

    let result = execute_tool(
        gateway.addr(),
        &workspace.root(),
        "code_extract_function",
        json!({
            "path": "src/lib.rs",
            "line": 5,
            "character": 5,
            "end_line": 5,
            "end_character": 23,
            "name": "calculate_value",
            "duplicates": true,
            "other_files": true,
            "apply": true,
        }),
    )
    .await
    .expect("the public extraction operation should return a plan");

    assert!(!result.is_error, "{}", text_of(&result));
    assert_eq!(workspace.read("src/shared.rs"), SHARED);
}
