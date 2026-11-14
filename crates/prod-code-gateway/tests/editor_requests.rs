/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #2018: an editor's requests to the in-memory Rust engine through the gateway's dispatch.

// The fixture serves several test targets; this one uses a part of it.
#[allow(dead_code)]
#[path = "common/native_rust.rs"]
mod native;

use native::{Fixture, SOURCE};
use serde_json::json;

/// `first` in `pub fn first() -> i32 { choose!(1) }`, the second line of the fixture.
fn highlight_first() -> serde_json::Value {
    json!({
        "textDocument": { "uri": "file:///client/src/lib.rs" },
        "position": { "line": 1, "character": 8 }
    })
}

/// An editor cancels the requests it no longer shows. One that has not begun, here because the
/// engine is busy, is answered `RequestCancelled` and never computed; before, it was computed
/// in its turn, ahead of the requests the editor still wanted.
#[tokio::test]
async fn a_request_cancelled_while_it_waits_is_not_computed() {
    let fixture = Fixture::new(SOURCE, "2021").await;
    let mut session = fixture.session(1).await;
    session.open(SOURCE).await;

    let engine = fixture.workspace.rust_engine.clone().unwrap();
    let busy = engine.lock().await;
    session
        .send(
            json!({"jsonrpc":"2.0", "id":7, "method":"textDocument/documentHighlight",
            "params": highlight_first()}),
        )
        .await;
    session
        .send(json!({"jsonrpc":"2.0", "method":"$/cancelRequest", "params":{"id":7}}))
        .await;
    drop(busy);
    let cancelled = session
        .receive()
        .await
        .expect("a cancelled request is answered");

    // The same request, not cancelled, is answered from a snapshot as before.
    let answered = session
        .request(
            json!(8),
            "textDocument/documentHighlight",
            highlight_first(),
        )
        .await;
    // A cancellation of a request already answered, or never made, is ignored.
    session
        .send(json!({"jsonrpc":"2.0", "method":"$/cancelRequest", "params":{"id":8}}))
        .await;
    let again = session
        .request(
            json!(9),
            "textDocument/documentHighlight",
            highlight_first(),
        )
        .await;
    fixture.finish(vec![session]).await;

    assert_eq!(cancelled["id"], 7, "{cancelled}");
    assert_eq!(cancelled["error"]["code"], -32800, "{cancelled}");
    for response in [answered, again] {
        assert!(
            response["result"]
                .as_array()
                .is_some_and(|ranges| !ranges.is_empty()),
            "{response}"
        );
    }
}
