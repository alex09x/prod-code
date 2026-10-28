/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;
use std::process::Command;

const GO_MOD: &str = "module example.com/api\n\ngo 1.22\n";
const STORE: &str = r#"package api

type Store struct {
    Name string
}

func (s Store) Price() int {
    return 7
}

func NewStore() Store { return Store{} }

func Decode(raw interface{}) Store {
    _ = raw
    return Store{}
}

func DecodeStruct(raw struct{}) Store {
    _ = raw
    return Store{}
}
"#;
const SHADOW: &str = r#"package api

func Local() int {
    type Store struct{ Magic int }
    return Store{}.Magic
}
"#;
const CONSUMER: &str = r#"package consumer

import api "example.com/api"

func NewStore() api.Store { return api.Store{} }
"#;
const OTHER: &str = r#"package other

import foreign "example.com/api/otherlib"

type Store struct{ Value int }

func NewOther() Store { return Store{} }
func NewForeign() foreign.Store { return foreign.Store{} }
"#;
const OTHERLIB: &str = r#"package otherlib

type Store struct{ Value int }
"#;

fn locations(path: &std::path::Path, line: u32, column: u32) -> Value {
    answers::locations(path, &[(line, column)])
}

async fn gateway_for_store_type(ws: &Workspace) -> std::net::SocketAddr {
    let store = ws.path("store.go");
    let shadow = ws.path("shadow.go");
    let other = ws.path("other/other.go");
    let otherlib = ws.path("otherlib/other.go");
    ScriptedGateway::start(move |method, params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/definition" => {
            let uri = params
                .pointer("/textDocument/uri")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if uri.ends_with("/store.go") {
                locations(&store, 3, 6)
            } else if uri.ends_with("/shadow.go") {
                locations(&shadow, 4, 10)
            } else if uri.ends_with("/consumer.go") {
                locations(&store, 3, 6)
            } else if uri.ends_with("/other.go") {
                let line = params
                    .pointer("/position/line")
                    .and_then(Value::as_u64)
                    .map(|line| line as u32 + 1)
                    .unwrap_or_default();
                if line == 7 {
                    locations(&other, 5, 6)
                } else if line == 8 {
                    locations(&otherlib, 2, 6)
                } else {
                    Value::Null
                }
            } else {
                Value::Null
            }
        }
        _ => Value::Null,
    })
    .await
    .addr()
}

fn text_of(result: &McpToolCallResult) -> String {
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
async fn public_go_extract_field_uses_type_identity_for_all_constructor_sites() {
    let ws = Workspace::new(&[
        ("go.mod", GO_MOD),
        ("store.go", STORE),
        ("shadow.go", SHADOW),
        ("consumer/consumer.go", CONSUMER),
        ("other/other.go", OTHER),
        ("otherlib/other.go", OTHERLIB),
        (
            "store_test.go",
            "package api\nimport \"testing\"\nfunc TestValues(t *testing.T) { if Local() != 0 { t.Fatal(Local()) }; if NewStore().Magic != 7 { t.Fatal(NewStore()) }; if Decode(nil).Magic != 7 { t.Fatal(Decode(nil)) }; if DecodeStruct(struct{}{}).Magic != 7 { t.Fatal(DecodeStruct(struct{}{})) } }\n",
        ),
        (
            "consumer/consumer_test.go",
            "package consumer\nimport \"testing\"\nfunc TestImportedStore(t *testing.T) { if got := NewStore().Magic; got != 7 { t.Fatal(got) } }\n",
        ),
        (
            "other/other_test.go",
            "package other\nimport \"testing\"\nfunc TestOtherStoreZero(t *testing.T) { if got := NewOther().Value; got != 0 { t.Fatal(got) }; if got := NewForeign().Value; got != 0 { t.Fatal(got) } }\n",
        ),
    ]);
    let remote = gateway_for_store_type(&ws).await;
    let result = prod_code_mcp::tools::execute_tool(
        remote,
        &ws.root(),
        "code_extract_field",
        serde_json::json!({
            "path": "store.go",
            "line": 8,
            "character": 12,
            "end_line": 8,
            "end_character": 13,
            "name": "Magic",
            "type": "int",
            "init": "7",
            "apply": true
        }),
    )
    .await
    .expect("public Go extraction should resolve selected type references");
    assert!(!result.is_error, "{}", text_of(&result));

    let store = ws.read("store.go");
    let shadow = ws.read("shadow.go");
    let consumer = ws.read("consumer/consumer.go");
    let other = ws.read("other/other.go");
    let otherlib = ws.read("otherlib/other.go");
    assert!(store.contains("Magic int"), "{store}");
    assert!(store.contains("Store{Magic: 7}"), "{store}");
    assert!(
        store.contains(
            "func Decode(raw interface{}) Store {\n    _ = raw\n    return Store{Magic: 7}"
        ),
        "{store}"
    );
    assert!(!store.contains("Magic: 7,\n    _ = raw"), "{store}");
    assert!(
        store.contains(
            "func DecodeStruct(raw struct{}) Store {\n    _ = raw\n    return Store{Magic: 7}"
        ),
        "{store}"
    );
    assert!(consumer.contains("api.Store{Magic: 7}"), "{consumer}");
    assert_eq!(
        shadow, SHADOW,
        "the local shadow Store literal is unrelated"
    );
    assert_eq!(other, OTHER, "a different package's Store is unrelated");
    assert_eq!(
        otherlib, OTHERLIB,
        "the imported package's Store is unrelated"
    );

    let output = Command::new("go")
        .args(["test", "./..."])
        .current_dir(ws.root())
        .output()
        .expect("run Go fixture tests");
    assert!(
        output.status.success(),
        "generated Go sources failed to compile:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn public_go_extract_field_refuses_unresolved_constructor_identity_even_with_force() {
    let store = "package api\n\ntype Store struct{}\n\
func (s Store) Price() int { return 7 }\n\
func NewStore() Store { return Store{} }\n";
    let ws = Workspace::new(&[("go.mod", GO_MOD), ("store.go", store)]);
    let file = ws.path("store.go");
    let remote = ScriptedGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/definition" => Value::Null,
        _ => Value::Null,
    })
    .await;

    let error = prod_code_mcp::tools::execute_tool(
        remote.addr(),
        &ws.root(),
        "code_extract_field",
        serde_json::json!({
            "path": "store.go",
            "expression": "7",
            "name": "Magic",
            "type": "int",
            "init": "7",
            "apply": true,
            "force": true
        }),
    )
    .await
    .expect_err("force must not write an unresolved constructor identity");

    assert!(
        format!("{error:#}").contains("nothing was written"),
        "{error:#}"
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), store);
}
