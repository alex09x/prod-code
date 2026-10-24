/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The field handlers called directly: every way to name a selection or a field gives the
//! same plan, a dry run writes nothing, and the compiler's verdict decides the write.

use super::*;
use crate::protocol::{McpContentItem, McpToolCallResult};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

const CALC: &str = "export class Calculator {\n    compute(factor: number): number {\n        return 100 * factor;\n    }\n}\n";
const SERVER: &str = "export class Server {\n    port: number;\n    host: string;\n}\n";

fn text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect()
}

async fn gateway(exit: i64, diagnostics: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/diagnostic" => diagnostics.clone(),
        "prod-code/shadow-run" => json!({ "exit_code": exit }),
        _ => Value::Null,
    })
    .await
}

#[tokio::test]
async fn every_spelling_of_a_selection_plans_the_same_extraction() {
    let ws = Workspace::new(&[("calc.ts", CALC)]);
    let root = ws.root();
    let gw = gateway(0, answers::no_diagnostics()).await;
    let run = |extra: Value| {
        let mut args = json!({ "path": "calc.ts", "name": "baseVal", "type": "number" });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        let addr = gw.addr();
        let root = root.clone();
        async move { handle_extract_field(addr, &root, &args).await }
    };
    let reference = run(json!({ "line": 3, "character": 16, "end_line": 3, "end_character": 19 }))
        .await
        .unwrap();
    assert!(!reference.is_error, "{}", text(&reference));
    for selection in [
        json!({ "range": "3:16-3:19" }),
        json!({ "range": " 3:16 .. 3:19 " }),
        json!({ "expression": "100" }),
        // an incomplete explicit selection falls through to the next way of naming one
        json!({ "line": 3, "character": 16, "range": "3:16-3:19" }),
    ] {
        let got = run(selection.clone()).await.unwrap();
        assert_eq!(text(&got), text(&reference), "{selection}");
    }
    assert_eq!(ws.read("calc.ts"), CALC, "a dry run writes nothing");

    let written = run(json!({ "expression": "100", "apply": true }))
        .await
        .unwrap();
    assert!(!written.is_error, "{}", text(&written));
    let content = ws.read("calc.ts");
    assert!(content.contains("baseVal: number = 100;"), "{content}");
    assert!(
        content.contains("return this.baseVal * factor;"),
        "{content}"
    );
}

#[tokio::test]
async fn a_selection_in_a_multibyte_line_is_found_by_utf16_column() {
    let source = "export class T {\n    f(): string {\n        return \"é😀\" + \"x\";\n    }\n}\n";
    let ws = Workspace::new(&[("t.ts", source)]);
    let gw = gateway(0, answers::no_diagnostics()).await;
    let done = handle_extract_field(
        gw.addr(),
        &ws.root(),
        &json!({ "path": "t.ts", "expression": "\"x\"", "name": "tail", "apply": true }),
    )
    .await
    .unwrap();
    assert!(!done.is_error, "{}", text(&done));
    let content = ws.read("t.ts");
    assert!(content.contains("tail"), "{content}");
    assert!(content.contains("\"é😀\" + this.tail"), "{content}");
}

fn field_args(extra: Value) -> Value {
    let mut args = json!({ "path": "server.ts" });
    for (key, value) in extra.as_object().unwrap() {
        args[key] = value.clone();
    }
    args
}

#[tokio::test]
async fn every_spelling_of_a_field_plans_the_same_encapsulation() {
    let ws = Workspace::new(&[("server.ts", SERVER)]);
    let root = ws.root();
    let gw = gateway(0, answers::no_diagnostics()).await;
    let reference =
        handle_encapsulate_field(gw.addr(), &root, &field_args(json!({ "field": "port" })))
            .await
            .unwrap();
    assert!(!reference.is_error, "{}", text(&reference));
    for spelling in [
        json!({ "field_name": "port", "class_name": "Server" }),
        json!({ "symbol": "Server::port" }),
        json!({ "symbol": "Server.port" }),
        json!({ "symbol": "port", "struct_name": "Server" }),
        json!({ "line": 2, "character": 6 }),
        json!({ "line": 2, "character": 5 }),
    ] {
        let got = handle_encapsulate_field(gw.addr(), &root, &field_args(spelling.clone()))
            .await
            .unwrap();
        assert_eq!(text(&got), text(&reference), "{spelling}");
    }
    assert_eq!(ws.read("server.ts"), SERVER, "a dry run writes nothing");

    // the character defaults to column 1, which on an indented line is on no name
    for args in [json!({ "line": 2 }), json!({ "line": 99, "character": 1 })] {
        let err = handle_encapsulate_field(gw.addr(), &root, &field_args(args.clone()))
            .await
            .unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "Could not find field at given line/character",
            "{args}"
        );
    }
}

#[tokio::test]
async fn the_compiler_decides_whether_an_encapsulation_is_written() {
    let args = field_args(json!({ "field": "port", "apply": true, "verify": "compile" }));
    let rejected = Workspace::new(&[("server.ts", SERVER)]);
    let gw = gateway(1, answers::no_diagnostics()).await;
    let done = handle_encapsulate_field(gw.addr(), &rejected.root(), &args)
        .await
        .unwrap();
    assert!(done.is_error, "{}", text(&done));
    assert!(text(&done).contains("nothing was written: the compiler rejects it"));
    assert_eq!(rejected.read("server.ts"), SERVER);

    let accepted = Workspace::new(&[("server.ts", SERVER)]);
    let gw = gateway(0, answers::no_diagnostics()).await;
    let done = handle_encapsulate_field(gw.addr(), &accepted.root(), &args)
        .await
        .unwrap();
    assert!(!done.is_error, "{}", text(&done));
    assert!(text(&done).contains("the compiler accepts the result too"));
    let content = accepted.read("server.ts");
    assert!(content.contains("private _port: number;"), "{content}");
    assert!(content.contains("public getPort(): number"), "{content}");
}
