/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::native::{Fixture, Session, document};
use super::support::{artifact, position};
use prod_code_gateway::admission::Admission;
use prod_code_gateway::workspace::SharedWorkspace;
use serde_json::{Value, json};
use std::sync::Arc;

const METHOD: &str = "textDocument/typeDefinition";
const SOURCE: &str = concat!(
    "// é🦀\r\n",
    "mod left { pub struct S; }\r\n",
    "mod right { pub struct S; }\r\n",
    "struct Pair<T>(T);\r\n",
    "impl left::S { fn f(s: &left::S) {} }\r\n",
    "fn sample() { /* 🦀 */ let a = left::S; let b = right::S; ",
    "let pair = Pair(left::S); let number = 1u32; ",
    "let _ = (&a, &b, &pair, &number); }\r\n",
);

fn params(source: &str, needle: &str) -> Value {
    let (line, col) = position(source, needle);
    let mut params = document();
    params["position"] = json!({"line":line-1,"character":col-1});
    params
}

async fn query(session: &mut Session, id: Value, params: Value, records: &mut Vec<Value>) -> Value {
    let reply = session.request(id.clone(), METHOD, params.clone()).await;
    records.push(json!({"request":{"id":id,"method":METHOD,"params":params},"response":reply}));
    artifact(
        "navigation",
        "replies.json",
        &serde_json::to_string_pretty(records).unwrap(),
    );
    assert_eq!(reply["id"], id);
    reply
}

fn location(source: &str, needle: &str) -> Value {
    let (line, col) = position(source, needle);
    json!({"uri":"file:///client/src/lib.rs", "range":{
        "start":{"line":line-1,"character":col-1},
        "end":{"line":line-1,"character":col}
    }})
}

#[tokio::test]
async fn native_type_navigation_has_strict_positions_and_session_identity() {
    let fixture = Fixture::new(SOURCE, "2021").await;
    let mut base = fixture.session(201).await;
    let mut records = Vec::new();
    let initialized = base.request(json!("caps"), "initialize", json!({})).await;
    assert_eq!(
        initialized["result"]["capabilities"]["typeDefinitionProvider"],
        true
    );
    records.push(initialized);
    let left = location(SOURCE, "S; }");
    let right = location(SOURCE, "S; }\r\nstruct Pair");
    for (id, needle, expected) in [
        (json!(0), "a, &b", json!([left.clone()])),
        (json!("same-spelling"), "b, &pair", json!([right])),
        (json!(203), "number);", json!([])),
    ] {
        let reply = query(&mut base, id, params(SOURCE, needle), &mut records).await;
        assert_eq!(reply["result"], expected, "{reply}");
    }
    let reply = query(
        &mut base,
        json!("multiple"),
        params(SOURCE, "pair, &number"),
        &mut records,
    )
    .await;
    assert_eq!(reply["result"].as_array().unwrap().len(), 2, "{reply}");
    assert!(reply["result"].as_array().unwrap().contains(&left));

    // Invalid positions must not clamp to a different, valid symbol.
    for position in [
        json!({}),
        json!({"line":-1,"character":0}),
        json!({"line":0,"character":1.5}),
        json!({"line":0,"character":4294967295_u64}),
        json!({"line":4294967296_u64,"character":0}),
        json!({"line":99,"character":0}),
        json!({"line":0,"character":999}),
        json!({"line":0,"character":5}), // middle of 🦀's UTF-16 pair
    ] {
        let mut p = document();
        p["position"] = position;
        let reply = query(&mut base, json!("invalid-position"), p, &mut records).await;
        assert_eq!(reply["error"]["code"], -32602, "{reply}");
    }
    for uri in [
        Value::Null,
        json!(9),
        json!("https://example.com/lib.rs"),
        json!("file://other-host/lib.rs"),
        json!("file:///client/missing.rs"),
    ] {
        let mut p = params(SOURCE, "a, &b");
        p["textDocument"]["uri"] = uri;
        let reply = query(&mut base, json!("invalid-document"), p, &mut records).await;
        assert_eq!(reply["error"]["code"], -32602, "{reply}");
    }
    for id in [None, Some(Value::Null)] {
        let mut request = json!({"jsonrpc":"2.0","method":METHOD,"params":params(SOURCE,"a, &b")});
        if let Some(id) = id {
            request["id"] = id;
        }
        base.send(request).await;
    }
    // No request IDs means no queued replies; the next answer must be this barrier.
    let reply = query(
        &mut base,
        json!("barrier"),
        params(SOURCE, "a, &b"),
        &mut records,
    )
    .await;
    assert_eq!(reply["result"], json!([left.clone()]));

    let worktree = fixture.temp.path().join("worktree");
    std::fs::create_dir_all(worktree.join("src")).unwrap();
    for file in ["Cargo.toml", "prod-code.toml"] {
        std::fs::copy(fixture.workspace.root.join(file), worktree.join(file)).unwrap();
    }
    let wt_source = format!("// worktree\r\n{SOURCE}");
    std::fs::write(worktree.join("src/lib.rs"), &wt_source).unwrap();
    fixture
        .workspace
        .rust_engine
        .as_ref()
        .unwrap()
        .lock()
        .await
        .attach_worktree(&worktree)
        .unwrap();
    let workspace = Arc::new(SharedWorkspace::with_base(
        worktree.clone(),
        "rust".into(),
        fixture.workspace.rust_engine.clone(),
        None,
        None,
        None,
        Some(Arc::clone(&fixture.workspace)),
    ));
    fixture
        .manager
        .insert_ready_for_test(Arc::clone(&workspace))
        .await;
    let mut wt = fixture.session_in(202, &workspace).await;
    let validation = workspace
        .validation_view(&Arc::new(Admission::unbounded()))
        .await
        .unwrap();
    assert!(validation.backend.is_none());
    let mut proposed = fixture.session_in(203, &workspace).await;
    let mut unchanged = fixture.session_in(204, &workspace).await;
    proposed.view.workspace = Arc::clone(&validation);
    unchanged.view.workspace = validation;
    let overlay = format!("// proposal é🦀\r\n{wt_source}");
    proposed.open(&overlay).await;
    for (session, source) in [
        (&mut proposed, overlay.as_str()),
        (&mut unchanged, wt_source.as_str()),
        (&mut wt, wt_source.as_str()),
        (&mut base, SOURCE),
    ] {
        let reply = query(
            session,
            json!("view"),
            params(source, "a, &b"),
            &mut records,
        )
        .await;
        assert_eq!(
            reply["result"],
            json!([location(source, "S; }")]),
            "{reply}"
        );
    }
    let reply = query(
        &mut proposed,
        json!("view-again"),
        params(&overlay, "a, &b"),
        &mut records,
    )
    .await;
    assert_eq!(reply["result"], json!([location(&overlay, "S; }")]));
    assert_eq!(
        std::fs::read_to_string(worktree.join("src/lib.rs")).unwrap(),
        wt_source
    );
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.root.join("src/lib.rs")).unwrap(),
        SOURCE
    );
    fixture.finish(vec![base, wt, proposed, unchanged]).await;
    artifact(
        "navigation",
        "replies.json",
        &serde_json::to_string_pretty(&records).unwrap(),
    );
}
