/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The extraction handlers called directly, below the symbol resolution `execute_tool` does
//! first: the refusals of `code_extract_interface`, and what `code_extract_interface` and
//! `code_extract_delegate` report for a dry run, a write and an analyzer that objects.

use super::*;
use crate::protocol::{McpContentItem, McpToolCallResult};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

const SERVICE_TS: &str = "export class UserService {\n    public getUser(id: string): User {\n        return findUser(id);\n    }\n\n    public saveUser(user: User): boolean {\n        return true;\n    }\n}\n";
const SERVICE_PY: &str = "class Service:\n    def __init__(self, host: str, port: int, name: str):\n        self.host = host\n        self.port = port\n        self.name = name\n\n    def url(self) -> str:\n        return f\"{self.host}:{self.port}\"\n";

fn text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect()
}

async fn gateway(diagnostics: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/diagnostic" => diagnostics.clone(),
        _ => Value::Null,
    })
    .await
}

fn nowhere() -> SocketAddr {
    "127.0.0.1:1".parse().unwrap()
}

#[tokio::test]
async fn extract_interface_names_the_missing_argument_and_the_rust_position_rule() {
    let ws = Workspace::new(&[("a.rs", "pub struct S;\n")]);
    let root = ws.root();
    let full = json!({ "symbol": "S", "interface_name": "I", "path": "a.rs" });
    let mut wrong = Vec::new();
    for (args, want) in [
        (json!({}), "Missing 'symbol' argument"),
        (
            json!({ "type_name": "S" }),
            "Missing 'interface_name' argument",
        ),
        (
            json!({ "symbol": "S", "name": "I" }),
            "Missing 'path' argument",
        ),
        (
            full.clone(),
            "Rust extract_interface requires one-based line and character positions",
        ),
        (
            json!({ "symbol": "S", "interface_name": "I", "path": "a.rs", "line": 1 }),
            "Rust extract_interface requires one-based line and character positions",
        ),
        (
            json!({ "symbol": "S", "interface_name": "I", "path": "a.rs", "character": 1 }),
            "Rust extract_interface requires one-based line and character positions",
        ),
    ] {
        match handle_extract_interface(nowhere(), &root, &args).await {
            Err(err) if format!("{err:#}") == want => {}
            other => wrong.push(format!(
                "{args}: {:?}",
                other.map_err(|e| format!("{e:#}")).map(|r| text(&r))
            )),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[tokio::test]
async fn extract_interface_previews_then_writes_the_interface() {
    let ws = Workspace::new(&[("service.ts", SERVICE_TS)]);
    let root = ws.root();
    let gw = gateway(answers::no_diagnostics()).await;
    let args = |extra: Value| {
        let mut args =
            json!({ "type_name": "UserService", "name": "IUserService", "path": "service.ts" });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    };
    let preview =
        handle_extract_interface(gw.addr(), &root, &args(json!({ "methods": ["getUser"] })))
            .await
            .unwrap();
    assert!(!preview.is_error, "{}", text(&preview));
    assert!(text(&preview).contains("getUser"), "{}", text(&preview));
    assert!(!text(&preview).contains("saveUser"), "{}", text(&preview));
    assert_eq!(
        ws.read("service.ts"),
        SERVICE_TS,
        "a dry run writes nothing"
    );

    let written = handle_extract_interface(gw.addr(), &root, &args(json!({ "apply": true })))
        .await
        .unwrap();
    assert!(!written.is_error, "{}", text(&written));
    let content = ws.read("service.ts");
    assert!(
        content.contains("export interface IUserService {"),
        "{content}"
    );
    assert!(
        content.contains("saveUser(user: User): boolean;"),
        "{content}"
    );
    assert!(
        content.contains("export class UserService implements IUserService {"),
        "{content}"
    );
}

#[tokio::test]
async fn extract_delegate_previews_writes_and_reports_an_objecting_analyzer() {
    let ws = Workspace::new(&[("service.py", SERVICE_PY)]);
    let root = ws.root();
    let gw = gateway(answers::no_diagnostics()).await;
    let args = |extra: Value| {
        let mut args = json!({ "path": "service.py", "fields": ["host", "port"],
            "methods": ["url"], "name": "Endpoint", "field": "endpoint" });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    };
    let preview = handle_extract_delegate(gw.addr(), &root, &args(json!({ "class": "Service" })))
        .await
        .unwrap();
    assert!(!preview.is_error, "{}", text(&preview));
    assert_eq!(
        ws.read("service.py"),
        SERVICE_PY,
        "a dry run writes nothing"
    );
    // the class can be named by `type` or by its position instead
    for named in [
        json!({ "type": "Service" }),
        json!({ "line": 1, "character": 7 }),
    ] {
        let again = handle_extract_delegate(gw.addr(), &root, &args(named.clone()))
            .await
            .unwrap();
        assert_eq!(text(&again), text(&preview), "{named}");
    }

    let objecting = gateway(answers::error_at(1, 1, "E1", "the analyzer objects")).await;
    let refused = handle_extract_delegate(
        objecting.addr(),
        &root,
        &args(json!({ "symbol": "Service", "apply": true })),
    )
    .await;
    match refused {
        Ok(done) => {
            assert!(done.is_error, "{}", text(&done));
            assert!(
                text(&done).contains("the analyzer objects"),
                "{}",
                text(&done)
            );
        }
        Err(err) => assert!(
            format!("{err:#}").contains("the analyzer objects"),
            "{err:#}"
        ),
    }
    assert_eq!(
        ws.read("service.py"),
        SERVICE_PY,
        "an objecting analyzer stops the write"
    );

    let written = handle_extract_delegate(
        gw.addr(),
        &root,
        &args(json!({ "symbol": "Service", "apply": true })),
    )
    .await
    .unwrap();
    assert!(!written.is_error, "{}", text(&written));
    let content = ws.read("service.py");
    assert!(
        content.contains("self.endpoint = Endpoint(host, port)"),
        "{content}"
    );
    assert!(content.contains("class Endpoint:"), "{content}");
}

const TWICE: &str = "pub fn a(x: u32) -> u32 {\n    let y = x * 2;\n    y + 1\n}\n\npub fn b(x: u32) -> u32 {\n    let y = x * 2;\n    y\n}\n";
const TWICE_EXTRACTED: &str = "pub fn a(x: u32) -> u32 {\n    let y = fun_name(x);\n    y + 1\n}\n\nfn fun_name(x: u32) -> u32 {\n    let y = x * 2;\n    y\n}\n\npub fn b(x: u32) -> u32 {\n    let y = x * 2;\n    y\n}\n";

async fn rust_gateway(path: &Path, exit: i64) -> ScriptedGateway {
    let assist = answers::whole_file(path, TWICE, TWICE_EXTRACTED);
    ScriptedGateway::start(move |method, _params| match method {
        "prodCode/applyAssist" => assist.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "prod-code/shadow-run" => json!({ "exit_code": exit }),
        _ => Value::Null,
    })
    .await
}

#[tokio::test]
async fn extract_function_asks_the_compiler_when_it_replaced_a_duplicate() {
    let args = |apply: bool, extra: Value| {
        let mut args = json!({ "path": "src/lib.rs", "line": 2, "character": 5,
            "end_line": 2, "end_character": 19, "name": "doubled", "apply": apply });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        args
    };
    let rejected = Workspace::new(&[("src/lib.rs", TWICE)]);
    let gw = rust_gateway(&rejected.path("src/lib.rs"), 1).await;
    let done = handle_extract_function(gw.addr(), &rejected.root(), &args(true, json!({})))
        .await
        .unwrap();
    assert!(done.is_error, "{}", text(&done));
    assert!(
        text(&done).contains("the compiler rejects the result"),
        "{}",
        text(&done)
    );
    assert_eq!(
        rejected.read("src/lib.rs"),
        TWICE,
        "a rejected duplicate replacement is not written"
    );

    let accepted = Workspace::new(&[("src/lib.rs", TWICE)]);
    let gw = rust_gateway(&accepted.path("src/lib.rs"), 0).await;
    let done = handle_extract_function(gw.addr(), &accepted.root(), &args(true, json!({})))
        .await
        .unwrap();
    assert!(!done.is_error, "{}", text(&done));
    assert!(
        text(&done).contains("the compiler accepts the result too"),
        "{}",
        text(&done)
    );
    let now = accepted.read("src/lib.rs");
    assert!(now.contains("fn doubled(x: u32) -> u32 {"), "{now}");
    assert_eq!(
        now.matches("doubled(x)").count(),
        2,
        "both copies call it: {now}"
    );

    // `verify: compile` asks the compiler even when nothing was replaced
    let alone = Workspace::new(&[("src/lib.rs", TWICE)]);
    let gw = rust_gateway(&alone.path("src/lib.rs"), 0).await;
    let done = handle_extract_function(
        gw.addr(),
        &alone.root(),
        &args(false, json!({ "duplicates": false, "verify": "compile" })),
    )
    .await
    .unwrap();
    assert!(
        text(&done).contains("the compiler accepts the result too"),
        "{}",
        text(&done)
    );
    assert_eq!(alone.read("src/lib.rs"), TWICE, "a dry run writes nothing");
}

const MULTIPLY: &str =
    "pub fn f(a: u32, b: u32) -> u32 {\n    let x = a * b + 1;\n    x + a * b\n}\n";

#[tokio::test]
async fn introduce_variable_previews_writes_and_reports_an_objecting_analyzer() {
    let args = |apply: bool| {
        json!({ "path": "src/lib.rs", "line": 2, "character": 13, "end_line": 2,
            "end_character": 18, "name": "product", "apply": apply })
    };
    let ws = Workspace::new(&[("src/lib.rs", MULTIPLY)]);
    let gw = gateway(answers::no_diagnostics()).await;
    let preview = handle_introduce_variable(gw.addr(), &ws.root(), &args(false))
        .await
        .unwrap();
    assert!(!preview.is_error, "{}", text(&preview));
    assert_eq!(ws.read("src/lib.rs"), MULTIPLY, "a dry run writes nothing");
    let written = handle_introduce_variable(gw.addr(), &ws.root(), &args(true))
        .await
        .unwrap();
    assert!(!written.is_error, "{}", text(&written));
    assert_eq!(
        ws.read("src/lib.rs"),
        "pub fn f(a: u32, b: u32) -> u32 {\n    let product = a * b;\n    let x = product + 1;\n    x + product\n}\n"
    );

    let objecting = Workspace::new(&[("src/lib.rs", MULTIPLY)]);
    let gw = gateway(answers::error_at(2, 5, "E0425", "cannot find value")).await;
    let refused = handle_introduce_variable(gw.addr(), &objecting.root(), &args(false))
        .await
        .unwrap();
    assert!(refused.is_error, "{}", text(&refused));
    assert!(
        text(&refused).contains("cannot find value"),
        "{}",
        text(&refused)
    );
}
