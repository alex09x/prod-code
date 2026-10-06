/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{AnyStream, ProdCodeCodec, WireMessage};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio_util::codec::Framed;

use super::super::pool::is_connection_error;
use super::super::types::{LspSession, budget_for};

/// Connects a session directly to a framed peer that replies once, without the testkit's
/// normal result wrapper. This keeps malformed JSON-RPC envelopes observable at the
/// session boundary.
async fn raw_session(reply: serde_json::Value) -> LspSession {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut peer = Framed::new(socket, ProdCodeCodec::new());
        let request = match peer.next().await {
            Some(Ok(WireMessage::LspPayload(json))) => {
                serde_json::from_str::<serde_json::Value>(&json).unwrap()
            }
            other => panic!("expected an LSP request, got {other:?}"),
        };
        let mut reply = reply;
        reply["jsonrpc"] = serde_json::json!("2.0");
        reply["id"] = request["id"].clone();
        peer.send(WireMessage::LspPayload(reply.to_string()))
            .await
            .unwrap();
    });
    LspSession {
        remote: addr,
        framed: Framed::new(
            AnyStream::connect(addr).await.unwrap(),
            ProdCodeCodec::new(),
        ),
        root: PathBuf::new(),
        opened: HashMap::new(),
        next_id: 1,
        engine: "test".to_string(),
        engine_loaded: None,
        index_gated: false,
    }
}

#[tokio::test]
async fn a_missing_result_is_an_actionable_method_error() {
    let mut session = raw_session(serde_json::json!({})).await;
    let error = session
        .request("textDocument/implementation", serde_json::json!({}))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(error, "textDocument/implementation response has no result");
}

#[tokio::test]
async fn explicit_null_and_ordinary_results_are_preserved() {
    for expected in [
        serde_json::Value::Null,
        serde_json::json!([]),
        serde_json::json!({ "found": true }),
        serde_json::json!(7),
    ] {
        let mut session = raw_session(serde_json::json!({ "result": expected.clone() })).await;
        assert_eq!(
            session
                .request("textDocument/implementation", serde_json::json!({}))
                .await
                .unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn an_error_envelope_remains_an_error() {
    let mut session = raw_session(serde_json::json!({
        "error": { "code": -32001, "message": "server refused" }
    }))
    .await;
    let error = session
        .request("textDocument/implementation", serde_json::json!({}))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(error, "textDocument/implementation failed: server refused");
}

#[test]
fn a_full_check_gets_more_time_than_an_interactive_query() {
    assert_eq!(budget_for("textDocument/hover").as_secs(), 60);
    assert_eq!(budget_for("textDocument/diagnostic").as_secs(), 300);
    assert_eq!(budget_for("prodCode/structuralReplace").as_secs(), 900);
}

#[test]
fn rebalance_error_is_classified_as_connection_error() {
    let err = anyhow::anyhow!("session rebalanced to 127.0.0.1:9400: congested gateway");
    assert!(is_connection_error(&err));
}
