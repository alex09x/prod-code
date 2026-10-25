/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn only_messages_with_an_id_and_a_method_are_server_requests() {
    for (json, expected) in [
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"workspace/configuration"}"#,
            true,
        ),
        (r#"{"id":"abc","method":"x"}"#, true),
        (r#"{"id":null,"method":"x"}"#, true),
        (r#"{"jsonrpc":"2.0","method":"window/logMessage"}"#, false),
        (r#"{"jsonrpc":"2.0","id":1,"result":null}"#, false),
        (r#"{"method":"x","params":{"note":"\"id\""}}"#, false),
        ("not json with \"id\" inside", false),
        ("", false),
    ] {
        assert_eq!(is_server_request(json), expected, "{json}");
    }
}

#[test]
fn the_fallback_answers_only_the_three_housekeeping_requests() {
    for (json, expected) in [
        (
            r#"{"id":1,"method":"window/workDoneProgress/create"}"#,
            true,
        ),
        (r#"{"id":"s","method":"workspace/configuration"}"#, true),
        (r#"{"id":0,"method":"client/registerCapability"}"#, true),
        (r#"{"id":null,"method":"workspace/configuration"}"#, false),
        (r#"{"method":"workspace/configuration"}"#, false),
        (r#"{"id":1,"method":"workspace/applyEdit"}"#, false),
        (r#"{"id":1,"method":5}"#, false),
        (r#"{"id":1}"#, false),
        ("[", false),
    ] {
        assert_eq!(fallback_answers_request(json), expected, "{json}");
    }
}

#[tokio::test]
async fn a_busy_note_is_sent_ahead_of_the_answer_and_removed_from_it() {
    let (raw_tx, mut raw_rx) = rapidfire::mpsc::bounded(4);
    let out = SharedOutputSender::new(raw_tx, std::time::Duration::from_secs(1));
    let member = prod_code_protocol::readiness::BUSY_MEMBER;
    let mut answer = serde_json::json!({"jsonrpc":"2.0","id":7,"result":[]});
    answer[member] = serde_json::json!({"reason":"indexing"});
    send_busy_note(&mut answer, &out).await;
    assert_eq!(
        answer,
        serde_json::json!({"jsonrpc":"2.0","id":7,"result":[]})
    );
    let WireMessage::LspPayload(note) = raw_rx.recv().await.unwrap().message else {
        panic!("expected a payload");
    };
    let note: serde_json::Value = serde_json::from_str(&note).unwrap();
    assert_eq!(
        note,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": prod_code_protocol::readiness::BUSY_NOTIFICATION,
            "params": {"reason": "indexing"}
        })
    );

    // An answer without a note, and a value that is not an object, send nothing.
    let mut plain = serde_json::json!({"id":8,"result":null});
    send_busy_note(&mut plain, &out).await;
    assert_eq!(plain, serde_json::json!({"id":8,"result":null}));
    let mut scalar = serde_json::json!(3);
    send_busy_note(&mut scalar, &out).await;
    drop(out);
    assert!(raw_rx.recv().await.is_err());
}
