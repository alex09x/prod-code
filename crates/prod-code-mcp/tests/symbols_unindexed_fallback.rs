/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `code_symbols` looks for a declaration the index omits whenever nothing of exactly the asked
//! name was found, fuzzy matches or not, and a declaration in a comment or a string literal is
//! not one (#1421).

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

const CARGO: &str = "[package]\nname = \"x\"\nversion = \"0.1.0\"\n";

async fn symbols(ws: &Workspace, query: &str, indexed: serde_json::Value) -> String {
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => indexed.clone(),
        _ => serde_json::Value::Null,
    })
    .await;
    let result = execute_tool(
        gateway.addr(),
        &ws.root(),
        "code_symbols",
        serde_json::json!({ "query": query }),
    )
    .await
    .expect("the search");
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
async fn fuzzy_matches_do_not_hide_an_unindexed_declaration_of_the_name() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", "pub struct LostBuilder;\n"),
        ("src/orphan.rs", "pub struct Lost {\n    pub at: u64,\n}\n"),
    ]);
    let lib = ws.path("src/lib.rs");
    let text = symbols(
        &ws,
        "Lost",
        serde_json::json!([answers::symbol("LostBuilder", 23, &lib, 1, 12)]),
    )
    .await;
    assert!(text.contains("LostBuilder"), "{text}");
    assert!(
        text.contains("`Lost` is declared at src/orphan.rs:1:12 (`pub struct Lost {`)"),
        "{text}"
    );
}

#[tokio::test]
async fn a_declaration_in_a_comment_or_a_string_is_not_one() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", "pub fn a() {}\n"),
        (
            "src/orphan.rs",
            "// fn Ghost() was here\nconst EXAMPLE: &str = \"fn Ghost()\";\npub fn Ghost() {}\n",
        ),
    ]);
    let text = symbols(&ws, "Ghost", serde_json::json!([])).await;
    assert!(
        text.contains("`Ghost` is declared at src/orphan.rs:3:8 (`pub fn Ghost() {}`)"),
        "{text}"
    );
    assert!(!text.contains("orphan.rs:1:"), "{text}");
    assert!(!text.contains("orphan.rs:2:"), "{text}");
}
