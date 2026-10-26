/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Asking a dependency's declaration for its checkout uses (#1419): a use that means another
//! item, a local of the same name, does not end the search of its file, and the position asked
//! is an LSP column in UTF-16 code units after supplementary characters too.

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::Arc;

const DECLARATION: &str = "/srv/registry/dep/src/lib.rs";
const CARGO: &str = "[package]\nname = \"x\"\nversion = \"0.1.0\"\n";

fn dep_widget() -> serde_json::Value {
    serde_json::json!([{
        "uri": format!("file://{DECLARATION}"),
        "range": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 17 } }
    }])
}

async fn references_of_dep_widget(ws: &Workspace, answer: prod_code_testkit::Answer) -> String {
    let remote = ScriptedGateway::start_arc(answer).await.addr();
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_references",
        serde_json::json!({ "path": DECLARATION, "line": 1, "character": 12 }),
    )
    .await
    .expect("the references");
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn a_local_of_the_same_name_does_not_end_the_search_of_its_file() {
    let source = "mod local {\n    pub struct Widget;\n}\nuse dep::Widget;\nfn main() { let _w = Widget; }\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/main.rs", source)]);
    let main = ws.path("src/main.rs");
    let answer = Arc::new(move |method: &str, params: &serde_json::Value| {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
        let line = params["position"]["line"].as_u64();
        match method {
            "prod-code/readFile" => serde_json::json!("pub struct Widget;\n"),
            // The first use is the local struct's own name: it means that struct.
            "textDocument/definition" if uri.ends_with("src/main.rs") && line == Some(1) => {
                answers::locations(&main, &[(2, 16)])
            }
            "textDocument/definition" if uri.ends_with("src/main.rs") => dep_widget(),
            "textDocument/references" if uri.ends_with("src/main.rs") => {
                answers::locations(&main, &[(4, 10), (5, 22)])
            }
            "textDocument/references" => serde_json::json!([]),
            _ => serde_json::Value::Null,
        }
    });
    let text = references_of_dep_widget(&ws, answer).await;
    assert!(
        text.contains("asked from a use of `Widget` in the checkout, src/main.rs:4:10"),
        "{text}"
    );
    assert!(text.contains("Found 2 reference(s)"), "{text}");
}

#[tokio::test]
async fn a_use_after_supplementary_characters_is_asked_at_its_utf16_column() {
    let line = "fn main() { let _s = \"😀😀\"; let _w = dep::Widget; }";
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/main.rs", &format!("{line}\n"))]);
    let main = ws.path("src/main.rs");
    let at = line.find("Widget").unwrap();
    let utf16 = line[..at].encode_utf16().count() as u64;
    assert_eq!(utf16, line[..at].chars().count() as u64 + 2);
    let answer = Arc::new(move |method: &str, params: &serde_json::Value| {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
        let character = params["position"]["character"].as_u64();
        match method {
            "prod-code/readFile" => serde_json::json!("pub struct Widget;\n"),
            // Only the real position of `Widget` is answered.
            "textDocument/definition"
                if uri.ends_with("src/main.rs") && character == Some(utf16) =>
            {
                dep_widget()
            }
            "textDocument/references"
                if uri.ends_with("src/main.rs") && character == Some(utf16) =>
            {
                answers::locations(&main, &[(1, utf16 as u32 + 1)])
            }
            "textDocument/references" => serde_json::json!([]),
            _ => serde_json::Value::Null,
        }
    });
    let text = references_of_dep_widget(&ws, answer).await;
    assert!(
        text.contains(&format!(
            "asked from a use of `Widget` in the checkout, src/main.rs:1:{}",
            utf16 + 1
        )),
        "{text}"
    );
}
