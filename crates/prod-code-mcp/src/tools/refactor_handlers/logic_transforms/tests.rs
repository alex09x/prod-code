/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `handle_generify` called directly, below the symbol resolution `execute_tool` does first:
//! how its own `symbol` argument becomes one declaration, or an explained refusal.

use super::*;
use crate::protocol::McpContentItem;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

const SERVICE: &str =
    "export function processItem(item: string): void {\n    console.log(item);\n}\n";
const GENERIC: &str = "export function processItem<T extends Printable>(item: T): void {\n    console.log(item);\n}\n";

async fn gateway(hits: Vec<Value>) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => Value::Array(hits.clone()),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
}

async fn refusal(gw: &ScriptedGateway, root: &Path, args: Value) -> String {
    match handle_generify(gw.addr(), root, &args).await {
        Err(err) => format!("{err:#}"),
        Ok(done) => panic!("{args} must be refused, got {:?}", done.content),
    }
}

#[tokio::test]
async fn a_symbol_becomes_one_declaration_or_an_explained_refusal() {
    let ws = Workspace::new(&[
        ("service.ts", SERVICE),
        ("other.ts", SERVICE),
        ("notes.txt", "processItem\n"),
    ]);
    let root = ws.root();
    let (service, other, notes) = (
        root.join("service.ts"),
        root.join("other.ts"),
        root.join("notes.txt"),
    );
    let args = json!({ "symbol": "processItem", "param": "item", "bound": "Printable" });
    let none = "no supported declaration for symbol processItem was found";

    assert_eq!(
        refusal(&gateway(Vec::new()).await, &root, args.clone()).await,
        none
    );
    // a hit in a file the tool cannot rewrite, or under another name, is not a declaration
    let near = gateway(vec![
        answers::symbol("processItem", 12, &notes, 1, 1),
        answers::symbol("processItemLater", 12, &service, 1, 17),
    ])
    .await;
    assert_eq!(refusal(&near, &root, args.clone()).await, none);
    let two = gateway(vec![
        answers::symbol("processItem", 12, &service, 1, 17),
        answers::symbol("PROCESSITEM", 12, &other, 1, 17),
    ])
    .await;
    assert_eq!(
        refusal(&two, &root, args.clone()).await,
        "multiple declarations for symbol processItem were found; pass a path to select one"
    );
    assert_eq!(ws.read("service.ts"), SERVICE);
    assert_eq!(ws.read("other.ts"), SERVICE);

    // one declaration, a qualified name matched by its last segment: written
    let one = gateway(vec![answers::symbol(
        "mod.processItem",
        12,
        &service,
        1,
        17,
    )])
    .await;
    let mut applied = args.clone();
    applied["apply"] = json!(true);
    let done = handle_generify(one.addr(), &root, &applied).await.unwrap();
    let McpContentItem::Text { text } = &done.content[0];
    assert!(!done.is_error, "{text}");
    assert_eq!(ws.read("service.ts"), GENERIC);
    assert_eq!(ws.read("other.ts"), SERVICE);

    let missing = handle_generify(one.addr(), &root, &json!({ "param": "item" })).await;
    assert_eq!(
        format!("{:#}", missing.unwrap_err()),
        "Missing 'path' or 'symbol' argument"
    );
}
