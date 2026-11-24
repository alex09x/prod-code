/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `impact`: a caller query whose connection the gateway closes is asked again on a new
//! session, instead of failing it and every later query of its language on the dead one
//! (#2076).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

const WAITER: &str = "tools/waiter.py";
const BEFORE: &str = "def wait_for_file(path):\n    return path\n\n\ndef run(path):\n    return wait_for_file(path)\n";
const AFTER: &str = "def wait_for_file(path):\n    return str(path)\n\n\ndef run(path):\n    return wait_for_file(path)\n";

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

#[tokio::test]
async fn a_caller_query_whose_connection_closed_is_asked_again_on_a_new_session() {
    let ws = Workspace::new(&[
        (
            "pyproject.toml",
            "[project]\nname = \"t\"\nversion = \"0.1.0\"\n",
        ),
        (WAITER, BEFORE),
    ]);
    ws.write(WAITER, AFTER);
    let uri = format!(
        "file://{}",
        std::fs::canonicalize(ws.root().join(WAITER))
            .unwrap()
            .display()
    );
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = asked.clone();
    let asked_run = Arc::new(AtomicUsize::new(0));
    let counter_run = asked_run.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => json!([
            answers::document_symbol("wait_for_file", 12, 1, 2, 5),
            answers::document_symbol("run", 12, 5, 6, 5),
        ]),
        "textDocument/prepareCallHierarchy" => {
            let line = params["position"]["line"].as_u64().unwrap_or(0);
            let (name, at) = if line >= 4 {
                ("run", 4)
            } else {
                ("wait_for_file", 0)
            };
            json!([{ "name": name, "kind": 12, "uri": uri,
                "range": { "start": { "line": at, "character": 0 }, "end": { "line": at + 1, "character": 0 } },
                "selectionRange": { "start": { "line": at, "character": 4 }, "end": { "line": at, "character": 4 + name.len() } } }])
        }
        "callHierarchy/incomingCalls" => {
            let item = params["item"]["name"].as_str().unwrap_or("");
            if item == "run" {
                // The walk to the next level loses its session once too.
                if counter_run.fetch_add(1, Ordering::SeqCst) == 0 {
                    return json!({ "disconnect": "the Python server restarted again" });
                }
                return json!([]);
            }
            if item != "wait_for_file" {
                return json!([]);
            }
            // The first time the gateway drops the session, as when its server is restarted.
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                return json!({ "disconnect": "the Python server restarted" });
            }
            json!([{ "from": { "name": "run", "kind": 12, "uri": uri,
                "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 5, "character": 0 } },
                "selectionRange": { "start": { "line": 4, "character": 4 }, "end": { "line": 4, "character": 7 } } },
                "fromRanges": [ { "start": { "line": 5, "character": 11 }, "end": { "line": 5, "character": 24 } } ] }])
        }
        _ => Value::Null,
    })
    .await;

    let text = text_of(
        &execute_tool(
            gateway.addr(),
            &ws.root(),
            "code_impact",
            json!({ "depth": 1, "budget_secs": 15 }),
        )
        .await
        .expect("the analysis runs"),
    );
    assert!(text.contains("run  tools/waiter.py:5:5"), "{text}");
    assert!(!text.contains("closed the session"), "{text}");
    assert!(asked.load(Ordering::SeqCst) >= 2, "asked again: {text}");
    assert!(
        asked_run.load(Ordering::SeqCst) >= 2,
        "run asked again: {text}"
    );
}
