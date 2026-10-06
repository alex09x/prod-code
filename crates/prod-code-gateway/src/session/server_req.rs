/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::shared_output::SharedOutputSender;
use crate::*;

/// Whether an LSP message is a request from the server (an id and a method), not a notification.
pub fn is_server_request(json: &str) -> bool {
    json.contains("\"id\"")
        && serde_json::from_str::<serde_json::Value>(json)
            .is_ok_and(|v| v.get("id").is_some() && v.get("method").is_some())
}

pub fn fallback_answers_request(json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json).is_ok_and(|value| {
        value.get("id").is_some_and(|id| !id.is_null())
            && matches!(
                value.get("method").and_then(serde_json::Value::as_str),
                Some(
                    "window/workDoneProgress/create"
                        | "workspace/configuration"
                        | "client/registerCapability"
                )
            )
    })
}

/// Sends the client the note an engine attached to an answer given while its server was still
/// loading or indexing, just before the answer, and takes it off the answer (#391).
pub async fn send_busy_note(resp: &mut serde_json::Value, out_tx: &SharedOutputSender) {
    let Some(busy) = resp
        .as_object_mut()
        .and_then(|o| o.remove(prod_code_protocol::readiness::BUSY_MEMBER))
    else {
        return;
    };
    let note = serde_json::json!({
        "jsonrpc": "2.0",
        "method": prod_code_protocol::readiness::BUSY_NOTIFICATION,
        "params": busy
    });
    let _ = out_tx.send(WireMessage::LspPayload(note.to_string())).await;
}
