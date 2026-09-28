//! Hierarchy failure shapes (#446): call hierarchy and non-Rust type hierarchy queries
//! report genuine remote errors and malformed responses instead of silently returning
//! empty results or "no hierarchy".

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{Answer, ScriptedGateway, Workspace, answers};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;

fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn scripted_gateway(answer: Answer) -> SocketAddr {
    ScriptedGateway::start_arc(answer).await.addr()
}

#[tokio::test]
async fn prepare_call_hierarchy_rejects_errors_and_malformations_and_preserves_empty_controls() {
    let ws = Workspace::empty();
    let lib = ws.write("src/lib.rs", "pub fn leaf() {}\n");
    ws.commit();
    let uri = format!("file://{}", lib.display());
    let valid_leaf = json!({
        "name": "leaf",
        "uri": uri.clone(),
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": 0, "character": 16 }
        },
        "selectionRange": {
            "start": { "line": 0, "character": 7 },
            "end": { "line": 0, "character": 11 }
        }
    });

    // 1. JSON-RPC failure
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/prepareCallHierarchy" => answers::failure("analyzer crashed"),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on rpc failure, got: {err:?}");

    // 2. Malformed non-array/non-null (object)
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/prepareCallHierarchy" => json!({ "unexpected": "object" }),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on non-array, got: {err:?}");

    // 3. Malformed item: empty object
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([{}]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on empty item, got: {err:?}");

    // 4. Malformed item: missing start position
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([{ "name": "leaf", "uri": uri }]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on missing position, got: {err:?}");

    // LSP coordinates outside the range this client can represent are malformed.
    let invalid_uri = format!("file://{}", lib.display());
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([{
            "name": "leaf",
            "uri": invalid_uri,
            "selectionRange": {
                "start": { "line": 4294967295u64, "character": 0 },
                "end": { "line": 4294967295u64, "character": 11 }
            }
        }]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on out-of-range coordinate, got: {err:?}");

    // A present but malformed selectionRange must not fall back to range.
    let invalid_uri = format!("file://{}", lib.display());
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([{
            "name": "leaf",
            "uri": invalid_uri,
            "selectionRange": null,
            "range": {
                "start": { "line": 0, "character": 7 },
                "end": { "line": 0, "character": 11 }
            }
        }]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on invalid selectionRange, got: {err:?}");

    // A call edge without the required fromRanges field is malformed, not a call with no sites.
    let leaf = valid_leaf.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([leaf]),
        "callHierarchy/incomingCalls" => json!([{ "from": {
            "name": "caller",
            "uri": "file:///caller.rs",
            "selectionRange": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 0, "character": 6 }
            }
        }}]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on missing fromRanges, got: {err:?}");

    // 5. Valid empty control: null
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/prepareCallHierarchy" => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    }))
    .await;
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("null prepareCallHierarchy succeeds");
    assert!(text_of(&res).contains("No function at"), "expected 'No function at', got: {}", text_of(&res));

    // 6. Valid empty control: []
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("empty array prepareCallHierarchy succeeds");
    assert!(text_of(&res).contains("No function at"), "expected 'No function at', got: {}", text_of(&res));
}

#[tokio::test]
async fn incoming_calls_expansion_rejects_errors_and_malformations_and_preserves_empty_controls() {
    let ws = Workspace::empty();
    let lib = ws.write("src/lib.rs", "pub fn leaf() {}\n");
    ws.commit();
    let uri = format!("file://{}", lib.display());
    let valid_leaf = json!({
        "name": "leaf",
        "uri": uri.clone(),
        "selectionRange": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 11 } }
    });

    // 1. JSON-RPC failure on incomingCalls
    let leaf = valid_leaf.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([leaf]),
        "callHierarchy/incomingCalls" => answers::failure("analyzer timeout"),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on rpc failure, got: {err:?}");

    // 2. Malformed non-array/non-null (object)
    let leaf = valid_leaf.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([leaf]),
        "callHierarchy/incomingCalls" => json!({ "error": "not array" }),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on non-array incomingCalls, got: {err:?}");

    // 3. Malformed edge: empty object
    let leaf = valid_leaf.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([leaf]),
        "callHierarchy/incomingCalls" => json!([{}]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on malformed edge, got: {err:?}");

    // 4. Malformed edge: invalid 'from' item
    let leaf = valid_leaf.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([leaf]),
        "callHierarchy/incomingCalls" => json!([{ "from": { "name": "bad" } }]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on malformed from item, got: {err:?}");

    // 5. Valid empty control: null
    let leaf = valid_leaf.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([leaf]),
        "callHierarchy/incomingCalls" => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    }))
    .await;
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("null incomingCalls succeeds");
    assert!(text_of(&res).contains("0 caller(s) — no callers found"), "expected 0 callers, got: {}", text_of(&res));

    // 6. Valid empty control: []
    let leaf = valid_leaf.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([leaf]),
        "callHierarchy/incomingCalls" => json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("empty incomingCalls succeeds");
    assert!(text_of(&res).contains("0 caller(s) — no callers found"), "expected 0 callers, got: {}", text_of(&res));
}

#[tokio::test]
async fn outgoing_calls_expansion_rejects_errors_and_malformations_and_preserves_empty_controls() {
    let ws = Workspace::empty();
    let lib = ws.write("src/lib.rs", "pub fn caller() {}\n");
    ws.commit();
    let uri = format!("file://{}", lib.display());
    let valid_caller = json!({
        "name": "caller",
        "uri": uri.clone(),
        "selectionRange": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 13 } }
    });

    // 1. JSON-RPC failure on outgoingCalls
    let caller = valid_caller.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([caller]),
        "callHierarchy/outgoingCalls" => answers::failure("analyzer error"),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callees",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on rpc failure, got: {err:?}");

    // 2. Malformed non-array/non-null
    let caller = valid_caller.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([caller]),
        "callHierarchy/outgoingCalls" => json!("bad shape"),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callees",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on non-array outgoingCalls, got: {err:?}");

    // 3. Malformed edge: missing 'to'
    let caller = valid_caller.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([caller]),
        "callHierarchy/outgoingCalls" => json!([{}]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_callees",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await;
    assert!(err.is_err(), "expected error on missing 'to', got: {err:?}");

    // 4. Valid empty control: null
    let caller = valid_caller.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([caller]),
        "callHierarchy/outgoingCalls" => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    }))
    .await;
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_callees",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("null outgoingCalls succeeds");
    assert!(text_of(&res).contains("0 callee(s) — no callees found"), "expected 0 callees, got: {}", text_of(&res));

    // 5. Valid empty control: []
    let caller = valid_caller.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareCallHierarchy" => json!([caller]),
        "callHierarchy/outgoingCalls" => json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_callees",
        json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("empty outgoingCalls succeeds");
    assert!(text_of(&res).contains("0 callee(s) — no callees found"), "expected 0 callees, got: {}", text_of(&res));
}

#[tokio::test]
async fn non_rust_prepare_type_hierarchy_rejects_errors_and_malformations_and_preserves_empty_controls() {
    let ws = Workspace::empty();
    let go = ws.write("shape.go", "package shape\n\ntype Square struct{}\n");
    ws.commit();
    let uri = format!("file://{}", go.display());
    let item = json!({
        "name": "Square",
        "kind": 23,
        "uri": format!("file://{}", go.display()),
        "range": {
            "start": { "line": 2, "character": 5 },
            "end": { "line": 2, "character": 11 }
        },
        "selectionRange": {
            "start": { "line": 2, "character": 5 },
            "end": { "line": 2, "character": 11 }
        }
    });

    // 1. JSON-RPC failure on prepareTypeHierarchy: must be an error, NOT "No type hierarchy at..."
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/prepareTypeHierarchy" => answers::failure("indexer crash"),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await;
    assert!(err.is_err(), "expected error on rpc failure, got: {err:?}");

    // 2. Malformed non-array/non-null (object)
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/prepareTypeHierarchy" => json!({ "not": "array" }),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await;
    assert!(err.is_err(), "expected error on non-array prepareTypeHierarchy, got: {err:?}");

    // 3. Malformed item: missing position
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareTypeHierarchy" => json!([{ "name": "Square", "uri": uri }]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await;
    assert!(err.is_err(), "expected error on malformed item, got: {err:?}");

    // 4. A malformed explicit selectionRange must not fall back to range.
    let malformed = json!({
        "name": "Square",
        "kind": 23,
        "uri": format!("file://{}", go.display()),
        "selectionRange": null,
        "range": {
            "start": { "line": 2, "character": 5 },
            "end": { "line": 2, "character": 11 }
        }
    });
    let prepared = item.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareTypeHierarchy" => json!([prepared]),
        "typeHierarchy/supertypes" => json!([malformed]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await;
    assert!(err.is_err(), "expected error on invalid supertype selectionRange, got: {err:?}");

    // 4. Valid empty control: null -> normal "answered none"
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/prepareTypeHierarchy" => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    }))
    .await;
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await
    .expect("null prepareTypeHierarchy succeeds");
    assert!(text_of(&res).contains("the language server answered none"), "expected 'answered none', got: {}", text_of(&res));

    // 5. Valid empty control: [] -> normal "answered none"
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/prepareTypeHierarchy" => json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await
    .expect("empty array prepareTypeHierarchy succeeds");
    assert!(text_of(&res).contains("the language server answered none"), "expected 'answered none', got: {}", text_of(&res));
}

#[tokio::test]
async fn non_rust_supertypes_rejects_errors_and_malformations_and_preserves_empty_controls() {
    let ws = Workspace::empty();
    let go = ws.write("shape.go", "package shape\n\ntype Square struct{}\n");
    ws.commit();
    let uri = format!("file://{}", go.display());
    let item = json!({
        "name": "Square",
        "kind": 23,
        "uri": uri.clone(),
        "range": { "start": { "line": 2, "character": 5 }, "end": { "line": 2, "character": 11 } },
        "selectionRange": { "start": { "line": 2, "character": 5 }, "end": { "line": 2, "character": 11 } }
    });

    // 1. JSON-RPC failure on typeHierarchy/supertypes
    let it = item.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareTypeHierarchy" => json!([it]),
        "typeHierarchy/supertypes" => answers::failure("failed resolving supertypes"),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await;
    assert!(err.is_err(), "expected error on rpc failure, got: {err:?}");

    // 2. Malformed non-array/non-null (object): must NOT return "`Square` has no supertype."
    let it = item.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareTypeHierarchy" => json!([it]),
        "typeHierarchy/supertypes" => json!({ "error": "malformed" }),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await;
    assert!(err.is_err(), "expected error on non-array supertypes, got: {err:?}");

    // 3. Malformed item in supertypes array
    let it = item.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareTypeHierarchy" => json!([it]),
        "typeHierarchy/supertypes" => json!([{}]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await;
    assert!(err.is_err(), "expected error on malformed supertype item, got: {err:?}");

    // A malformed line that would overflow when rendered 1-based is refused before conversion.
    let it = item.clone();
    let super_uri = uri.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareTypeHierarchy" => json!([it]),
        "typeHierarchy/supertypes" => json!([{
            "name": "Shape",
            "uri": super_uri,
            "selectionRange": {
                "start": { "line": 4294967295u64, "character": 5 },
                "end": { "line": 4294967295u64, "character": 10 }
            }
        }]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await;
    assert!(err.is_err(), "expected error on overflowing hierarchy position, got: {err:?}");

    // 4. Valid empty control: null -> "`Square` has no supertype."
    let it = item.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareTypeHierarchy" => json!([it]),
        "typeHierarchy/supertypes" => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    }))
    .await;
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await
    .expect("null supertypes succeeds");
    assert!(text_of(&res).contains("`Square` has no supertype"), "expected 'has no supertype', got: {}", text_of(&res));

    // 5. Valid empty control: [] -> "`Square` has no supertype."
    let it = item.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareTypeHierarchy" => json!([it]),
        "typeHierarchy/supertypes" => json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await
    .expect("empty array supertypes succeeds");
    assert!(text_of(&res).contains("`Square` has no supertype"), "expected 'has no supertype', got: {}", text_of(&res));

    // 6. Valid populated control: [Shape] -> "`Square` has 1 supertype(s):\n  • Shape  shape.go:10:6"
    let it = item.clone();
    let shape = json!([{
        "name": "Shape",
        "kind": 11,
        "uri": uri.clone(),
        "range": { "start": { "line": 9, "character": 5 }, "end": { "line": 9, "character": 10 } },
        "selectionRange": { "start": { "line": 9, "character": 5 }, "end": { "line": 9, "character": 10 } }
    }]);
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/prepareTypeHierarchy" => json!([it]),
        "typeHierarchy/supertypes" => shape.clone(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let res = execute_tool(
        remote,
        &ws.root(),
        "code_supertypes",
        json!({ "path": "shape.go", "line": 3, "character": 6 }),
    )
    .await
    .expect("valid supertypes succeeds");
    assert!(text_of(&res).contains("`Square` has 1 supertype(s)"), "expected 'has 1 supertype(s)', got: {}", text_of(&res));
    assert!(text_of(&res).contains("• Shape  shape.go:10:6"), "expected '• Shape  shape.go:10:6', got: {}", text_of(&res));
}
