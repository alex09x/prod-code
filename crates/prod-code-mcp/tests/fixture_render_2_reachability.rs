/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use prod_code_mcp::dead_code::{DeadCodeOptions, find_dead_code_opts};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

fn workspace() -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        ),
        ("src/lib.rs", "fn work() {}\n"),
    ])
}

#[tokio::test]
async fn malformed_references_preserve_symbols_with_exact_reasons() {
    let range = json!({"start": {"line": 0, "character": 0},
        "end": {"line": 0, "character": 1}});
    for (reference, expected) in [
        (json!({}), "a reference location has no URI"),
        (
            json!({"uri": "bad uri"}),
            "a reference location has an invalid URI `bad uri`",
        ),
        (
            json!({"uri": "file:///outside.rs"}),
            "a reference location for `file:///outside.rs` has no range",
        ),
        (
            json!({"uri": "file:///outside.rs", "range": {}}),
            "a reference location for `file:///outside.rs` has no range start",
        ),
        (
            json!({"uri": "file:///outside.rs", "range": {"start": {}}}),
            "a reference location for `file:///outside.rs` has no range end",
        ),
        (
            json!({"uri": "file:///outside.rs", "range": {"start": {}, "end": {}}}),
            "a reference location for `file:///outside.rs` has malformed coordinates",
        ),
        (
            json!({"uri": "file:///outside.rs", "range": {"start": {"line": 2, "character": 0}, "end": {"line": 1, "character": 0}}}),
            "a reference location for `file:///outside.rs` has a reversed range",
        ),
    ] {
        let ws = workspace();
        let gateway = ScriptedGateway::start(move |method, _| match method {
            "textDocument/documentSymbol" => json!([answers::document_symbol("work", 12, 1, 1, 4)]),
            "textDocument/references" => json!([reference.clone()]),
            _ => Value::Null,
        })
        .await;
        let r = find_dead_code_opts(
            gateway.addr(),
            &ws.root(),
            DeadCodeOptions {
                reachability: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            (r.files_scanned, r.symbols_checked, r.dead.len()),
            (1, 1, 0)
        );
        assert_eq!(r.unverified.len(), 1);
        let item = &r.unverified[0];
        assert_eq!(
            (
                item.file.as_str(),
                item.name.as_deref(),
                item.line,
                item.col
            ),
            ("src/lib.rs", Some("work"), 1, 4)
        );
        assert_eq!(item.reason, expected);
    }
    let ws = workspace();
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => json!([answers::document_symbol("work", 12, 1, 1, 4)]),
        "textDocument/references" => json!([{"uri": "file:///outside.rs", "range": range}]),
        _ => Value::Null,
    })
    .await;
    let r = find_dead_code_opts(
        gateway.addr(),
        &ws.root(),
        DeadCodeOptions {
            reachability: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(r.dead.len(), 0);
    assert_eq!(r.unverified.len(), 1);
    assert_eq!(
        r.unverified[0].reason,
        "reference at /outside.rs:1:1 cannot be attributed to a scanned caller; the symbol is treated as a possible root"
    );
}

#[tokio::test]
async fn unknown_document_shapes_do_not_claim_dead_symbols() {
    for answer in [Value::Null, json!({}), json!([{"kind": 12}])] {
        let ws = workspace();
        let gateway = ScriptedGateway::start(move |method, _| match method {
            "textDocument/documentSymbol" => answer.clone(),
            "textDocument/references" => {
                panic!("no reference query may follow an unknown symbol list")
            }
            _ => Value::Null,
        })
        .await;
        let r = find_dead_code_opts(
            gateway.addr(),
            &ws.root(),
            DeadCodeOptions {
                reachability: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            (r.files_scanned, r.symbols_checked, r.dead.len()),
            (0, 0, 0)
        );
        assert_eq!(r.unverified.len(), 1);
        assert_eq!(
            (
                r.unverified[0].name.as_deref(),
                r.unverified[0].line,
                r.unverified[0].col
            ),
            (None, 0, 0)
        );
        assert!(!r.complete());
    }
}
