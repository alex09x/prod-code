/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A function reached first deep and then higher up is expanded on the higher path, so
//! nothing within the requested depth goes missing; a real cycle is still a repeat (#1389).

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};

/// One function per line, in this order.
const NAMES: [&str; 6] = ["root", "a", "b", "c", "x", "y"];

fn item(uri: &str, name: &str) -> Value {
    let line = NAMES.iter().position(|n| *n == name).unwrap();
    json!({
        "name": name, "uri": uri, "kind": 12,
        "selectionRange": {"start": {"line": line, "character": 7},
                           "end": {"line": line, "character": 7 + name.len()}}
    })
}

/// `root -> a, b`, `a -> c`, `c -> x`, `b -> x`, `x -> y`, and `y -> root` when `cycle` is set.
async fn gateway(uri: String, cycle: bool) -> ScriptedGateway {
    ScriptedGateway::start(move |method, params| match method {
        "textDocument/prepareCallHierarchy" => json!([item(&uri, "root")]),
        "callHierarchy/outgoingCalls" => {
            let callees: &[&str] = match params["item"]["name"].as_str().unwrap_or("") {
                "root" => &["a", "b"],
                "a" => &["c"],
                "c" | "b" => &["x"],
                "x" => &["y"],
                "y" if cycle => &["root"],
                _ => &[],
            };
            Value::Array(
                callees
                    .iter()
                    .map(|name| {
                        let to = item(&uri, name);
                        json!({"to": to.clone(), "fromRanges": [to["selectionRange"].clone()]})
                    })
                    .collect(),
            )
        }
        _ => Value::Null,
    })
    .await
}

async fn callees(cycle: bool, depth: u64) -> String {
    let source: String = NAMES
        .iter()
        .map(|n| format!("pub fn {n}() {{}}\n"))
        .collect();
    let ws = Workspace::new(&[("src/lib.rs", source.as_str())]);
    let uri = url::Url::from_file_path(ws.path("src/lib.rs"))
        .unwrap()
        .to_string();
    let gw = gateway(uri, cycle).await;
    let args = json!({"path": "src/lib.rs", "line": 1, "character": 8, "depth": depth});
    let result = execute_tool(gw.addr(), &ws.root(), "code_callees", args)
        .await
        .expect("callees run");
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect()
}

#[tokio::test]
async fn a_shorter_later_path_expands_what_the_deeper_one_could_not() {
    // root(0) -> a(1) -> c(2) -> x(3), then root -> b(1) -> x(2) -> y(3).
    let text = callees(false, 3).await;
    assert!(
        text.contains(" y"),
        "y is three levels down through b and x: {text}"
    );
}

#[tokio::test]
async fn a_cycle_back_to_the_root_is_still_a_repeat() {
    let text = callees(true, 5).await;
    let roots = text.matches(" root").count();
    assert!(roots <= 2, "the cycle stops at the first repeat: {text}");
    assert!(text.contains(" y"), "{text}");
}
