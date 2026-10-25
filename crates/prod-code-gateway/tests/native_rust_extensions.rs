/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "native_rust_extensions/consumer.rs"]
mod consumer;
#[path = "common/native_rust.rs"]
mod native;
#[path = "native_rust_extensions/views.rs"]
mod views;

use native::{Fixture, SOURCE, document};
use serde_json::{Value, json};

#[tokio::test]
async fn native_extensions_answer_through_gateway_dispatch() {
    let fixture = Fixture::new(SOURCE, "2021").await;
    let mut session = fixture.session(1).await;
    let mut observed = Vec::new();
    for (id, method, params) in [
        (
            json!("syntax-baseline"),
            "rust-analyzer/viewSyntaxTree",
            document(),
        ),
        (json!(1352), "rust-analyzer/expandMacro", {
            let mut p = document();
            p["position"] = json!({"line":1, "character":24});
            p
        }),
    ] {
        session
            .send(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
            .await;
        observed.push((id, session.receive().await));
    }
    fixture.finish(vec![session]).await;
    for (id, response) in observed {
        let response = response.expect("native endpoint silently dropped the request");
        assert_eq!(response["id"], id);
        assert!(response.get("result").is_some(), "{response}");
    }
}

fn check_tree(tree: &Value, source: &str) {
    fn walk(node: &Value, source: &str, tokens: &mut String) {
        let start = node["start"][0].as_u64().unwrap() as usize;
        let end = node["end"][0].as_u64().unwrap() as usize;
        assert!(start <= end && end <= source.len());
        assert!(source.is_char_boundary(start) && source.is_char_boundary(end));
        for (position, byte) in [(&node["start"], start), (&node["end"], end)] {
            let prefix = &source[..byte];
            assert_eq!(position[1], prefix.bytes().filter(|&b| b == b'\n').count());
            assert_eq!(position[2], prefix.rsplit('\n').next().unwrap().len());
        }
        let kind = node["kind"].as_str().unwrap();
        assert_eq!(kind, kind.to_ascii_uppercase());
        if node["type"] == "Token" {
            tokens.push_str(&source[start..end]);
        } else {
            let mut at = start;
            for child in node["children"].as_array().unwrap() {
                assert_eq!(child["start"][0], at);
                walk(child, source, tokens);
                at = child["end"][0].as_u64().unwrap() as usize;
            }
            assert_eq!(at, end);
        }
    }
    assert_eq!(tree["kind"], "SOURCE_FILE");
    assert_eq!(tree["start"], json!([0, 0, 0]));
    assert_eq!(tree["end"][0], source.len());
    let mut tokens = String::new();
    walk(tree, source, &mut tokens);
    assert_eq!(tokens, source);
}

fn expanded(reply: Value, expected: &str) {
    assert_eq!(reply["result"]["name"], "choose!");
    let text = reply["result"]["expansion"].as_str().unwrap();
    assert_eq!(
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>(),
        expected
    );
}

#[tokio::test]
async fn native_trees_and_macros_use_private_session_overlays() {
    let fixture = Fixture::new(SOURCE, "2021").await;
    let mut a = fixture.session(11).await;
    let mut b = fixture.session(12).await;
    let mut base = fixture.session(13).await;
    let proposed = SOURCE
        .replace("+ 10", "+ 37")
        .replace("* 20", "* 43")
        .replace("pub fn first", "// é🦀\npub fn first")
        .replace("choose!(1)", "/* é🦀 */ choose!(1)")
        .replace('\n', "\r\n");
    let other = SOURCE.replace("+ 10", "+ 99");
    a.open(&proposed).await;
    b.open(&other).await;
    for _ in 0..2 {
        check_tree(&a.tree(json!("proposed-tree")).await, &proposed);
        expanded(
            a.expand(json!("outer"), &proposed, "choose!(1)").await,
            "1+37",
        );
        expanded(
            a.expand(json!("shadowed"), &proposed, "choose!(2)").await,
            "2*43",
        );
        check_tree(&b.tree(json!(22)).await, &other);
        expanded(b.expand(json!(23), &other, "choose!(1)").await, "1+99");
        check_tree(&base.tree(json!(24)).await, SOURCE);
        expanded(base.expand(json!(25), SOURCE, "choose!(1)").await, "1+10");
    }
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.root.join("src/lib.rs")).unwrap(),
        SOURCE
    );
    a.send(json!({"jsonrpc":"2.0", "method":"textDocument/didClose", "params":document()}))
        .await;
    check_tree(&a.tree(json!("closed")).await, SOURCE);
    expanded(
        a.expand(json!("restored"), SOURCE, "choose!(1)").await,
        "1+10",
    );
    fixture.finish(vec![a, b, base]).await;
}

#[tokio::test]
async fn native_extensions_bound_invalid_inputs_and_unavailable_expansions() {
    let fixture = Fixture::new(SOURCE, "2021").await;
    let mut session = fixture.session(31).await;
    for position in [
        json!({}),
        json!({"line":-1,"character":0}),
        json!({"line":0,"character":4294967296_u64}),
        json!({"line":99,"character":0}),
        json!({"line":0,"character":999}),
        json!({"line":0,"character":1.5}),
    ] {
        let mut params = document();
        params["position"] = position;
        let reply = session
            .request(json!("invalid"), "rust-analyzer/expandMacro", params)
            .await;
        assert_eq!(reply["error"]["code"], -32602, "{reply}");
    }
    let reply = session
        .expand(json!("no-macro"), SOURCE, "pub fn first")
        .await;
    assert_eq!(reply["result"], Value::Null);
    assert!(reply.get("error").is_none());
    for method in ["rust-analyzer/viewSyntaxTree", "rust-analyzer/expandMacro"] {
        let reply = session.request(json!(0), method, json!({})).await;
        assert_eq!(reply["error"]["code"], -32602, "{reply}");
    }
    let unicode = "// 🦀\r\nfn main() {}\r\n";
    session.open(unicode).await;
    for character in [4, 6] {
        // inside the surrogate pair, and inside CRLF
        let mut params = document();
        params["position"] = json!({"line":0, "character":character});
        let reply = session
            .request(
                json!("unicode-invalid"),
                "rust-analyzer/expandMacro",
                params,
            )
            .await;
        assert_eq!(reply["error"]["code"], -32602, "{reply}");
    }
    let reply = session
        .request(
            json!("missing-file"),
            "rust-analyzer/viewSyntaxTree",
            json!({"textDocument":{"uri":"file:///client/src/missing.rs"}}),
        )
        .await;
    assert_eq!(reply["error"]["code"], -32602);
    session.open("fn main() { unavailable!(0); }\n").await;
    let reply = session
        .expand(
            json!("unresolved"),
            "fn main() { unavailable!(0); }\n",
            "unavailable!",
        )
        .await;
    assert_eq!(reply["result"], Value::Null);
    assert!(reply.get("error").is_none());
    session.open(unicode).await;
    // Lock acquisition is part of the native deadline, and must produce an RPC error.
    let engine = fixture.workspace.rust_engine.as_ref().unwrap();
    let held = engine.lock().await;
    let mut params = document();
    params["prodCodeTimeoutMs"] = json!(10);
    let reply = session
        .request(json!("bounded"), "rust-analyzer/viewSyntaxTree", params)
        .await;
    assert_eq!(reply["error"]["code"], -32603);
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
    drop(held);
    for id in [None, Some(Value::Null)] {
        let mut notification =
            json!({"jsonrpc":"2.0", "method":"rust-analyzer/viewSyntaxTree", "params":document()});
        if let Some(id) = id {
            notification["id"] = id;
        }
        session.send(notification).await;
    }
    // A following request must receive its own answer, with no fabricated id=1 response.
    check_tree(&session.tree(json!("after-notifications")).await, unicode);
    fixture.finish(vec![session]).await;
}
