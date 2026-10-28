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
use std::process::Command;

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

#[tokio::test]
async fn moving_a_function_to_a_new_child_module_keeps_parent_scope_dependencies() {
    let workspace = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"child-scope\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub mod parent;\n"),
        (
            "src/parent.rs",
            "fn helper() -> usize { 1 }\nfn selected() -> usize { helper() }\n",
        ),
    ]);
    let gateway = ScriptedGateway::start(|method, _| match method {
        "textDocument/documentSymbol" => json!([
            {
                "name": "helper",
                "kind": 12,
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 26 } },
                "selectionRange": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 9 } }
            },
            {
                "name": "selected",
                "kind": 12,
                "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 42 } },
                "selectionRange": { "start": { "line": 1, "character": 3 }, "end": { "line": 1, "character": 11 } }
            }
        ]),
        "textDocument/references" => Value::Array(Vec::new()),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;

    let result = execute_tool(
        gateway.addr(),
        &workspace.root(),
        "code_move",
        json!({
            "path": "src/parent.rs",
            "line": 2,
            "character": 4,
            "to": "src/parent/selected.rs",
            "apply": true,
        }),
    )
    .await
    .unwrap_or_else(|error| panic!("code_move: {error:#}"));
    assert!(!result.is_error, "{}", text_of(&result));

    let output = Command::new("rustc")
        .args([
            "--edition=2021",
            "--crate-type=lib",
            "--crate-name=child_scope",
        ])
        .arg(workspace.path("src/lib.rs"))
        .arg("-o")
        .arg(workspace.root().join("libchild_scope.rlib"))
        .output()
        .expect("run the Rust compiler against the returned workspace");
    assert!(
        output.status.success(),
        "moved workspace must compile:\n{}\nparent={}\nchild={}",
        String::from_utf8_lossy(&output.stderr),
        workspace.read("src/parent.rs"),
        workspace.read("src/parent/selected.rs")
    );
    assert!(
        workspace
            .read("src/parent/selected.rs")
            .contains("use super::*;"),
        "{}",
        workspace.read("src/parent/selected.rs")
    );
}
