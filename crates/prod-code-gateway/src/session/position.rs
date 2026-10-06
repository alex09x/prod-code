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

/// What the session loop does once a client message has been handled.
#[derive(Debug, PartialEq, Eq)]
pub enum Flow {
    /// Wait for the next message.
    Next,
    /// The client is gone or asked to disconnect: end the session.
    Stop,
}

/// Decode an LSP's zero-based position into the one-based coordinates used by the Rust engine.
///
/// LSP positions must be non-negative JSON integers. The engine's one-based API also means
/// that `u32::MAX` cannot be represented, so reject it rather than truncating or overflowing.
pub fn one_based_position(
    position: Option<&serde_json::Value>,
) -> Result<(u32, u32), &'static str> {
    let position = position.ok_or("position is required")?;
    let coordinate = |name| {
        position
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .and_then(|value| value.checked_add(1))
            .ok_or("position coordinates must be non-negative integers below 4294967295")
    };
    Ok((coordinate("line")?, coordinate("character")?))
}

/// Keep request metrics bounded even when a forwarded request has an arbitrary JSON shape.
pub fn metric_position(position: Option<&serde_json::Value>) -> (u32, u32) {
    one_based_position(position).unwrap_or((1, 1))
}

/// Validate only the Rust methods that consume LSP positions locally.
pub fn native_position_params(
    method: Option<&str>,
    params: Option<&serde_json::Value>,
) -> Result<(), String> {
    let params = params.unwrap_or(&serde_json::Value::Null);
    match method {
        Some(
            "textDocument/hover"
            | "textDocument/definition"
            | "textDocument/references"
            | "textDocument/implementation"
            | "textDocument/prepareCallHierarchy"
            | "prodCode/safeDelete"
            | "textDocument/rename",
        ) => one_based_position(params.get("position"))
            .map(|_| ())
            .map_err(str::to_owned),
        Some("callHierarchy/incomingCalls" | "callHierarchy/outgoingCalls") => one_based_position(
            params
                .get("item")
                .and_then(|item| item.get("selectionRange"))
                .and_then(|range| range.get("start")),
        )
        .map(|_| ())
        .map_err(|reason| format!("item.selectionRange.start: {reason}")),
        Some("prodCode/structuralReplace") => params
            .get("position")
            .map(|position| {
                one_based_position(Some(position))
                    .map(|_| ())
                    .map_err(str::to_owned)
            })
            .unwrap_or(Ok(())),
        Some("prodCode/assists" | "prodCode/applyAssist") => {
            one_based_position(params.pointer("/range/start"))
                .map_err(|reason| format!("range.start: {reason}"))?;
            if let Some(end) = params.pointer("/range/end") {
                one_based_position(Some(end)).map_err(|reason| format!("range.end: {reason}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub async fn send_invalid_params(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    id: &serde_json::Value,
    method: &str,
    reason: &str,
) {
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32602,
            "message": format!("invalid params for {method}: {reason}"),
        }
    });
    let client_response = translator.translate_lsp_to_client(&response.to_string());
    let _ = out_tx.send(WireMessage::LspPayload(client_response)).await;
}
