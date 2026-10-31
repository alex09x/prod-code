/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::state::is_idempotent_lsp_request;
use super::transport::PendingRequests;
use anyhow::Result;
use futures_util::{SinkExt, stream::SplitSink};
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use std::collections::VecDeque;
use tokio_util::codec::Framed;

fn replay_request(method: &str, request: &str) -> Option<String> {
    if method != "textDocument/semanticTokens/full/delta" {
        return Some(request.to_string());
    }
    // A result id belongs to the old server. The delta response type also permits
    // a full token result, so ask the new server for a fresh baseline instead.
    let mut value: serde_json::Value = serde_json::from_str(request).ok()?;
    value
        .get_mut("params")?
        .as_object_mut()?
        .remove("previousResultId");
    value["method"] = serde_json::json!("textDocument/semanticTokens/full");
    Some(value.to_string())
}

pub(super) async fn replay_pending_requests(
    socket: &mut SplitSink<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>, WireMessage>,
    files: &prod_code_client::editor_files::RemoteFiles,
    pending_requests: &PendingRequests,
    editor_out: &tokio::sync::Mutex<tokio::io::Stdout>,
    deferred: &VecDeque<String>,
) -> Result<()> {
    // The new server has only the snapshot from before these edits. Replaying a
    // query now could return an answer about text the editor has already changed.
    let changed = deferred.iter().any(|frame| {
        matches!(
            prod_code_client::editor_files::method_of(frame).as_deref(),
            Some("textDocument/didOpen" | "textDocument/didChange" | "textDocument/didClose")
        )
    });
    let cancelled: Vec<serde_json::Value> = deferred
        .iter()
        .filter_map(|frame| {
            let message: serde_json::Value = serde_json::from_str(frame).ok()?;
            (message.get("method")?.as_str()? == "$/cancelRequest")
                .then(|| message.pointer("/params/id").cloned())
                .flatten()
        })
        .collect();
    let pending = {
        let mut pending = pending_requests.lock().await;
        pending.retain(|(id, _, _)| !cancelled.contains(id));
        pending.clone()
    };
    let mut rejected = Vec::new();
    for (id, method, request) in &pending {
        let idempotent = is_idempotent_lsp_request(method);
        if idempotent
            && !changed
            && let Some(request) = replay_request(method, request)
        {
            socket
                .send(WireMessage::LspPayload(files.to_node(&request)))
                .await?;
        } else {
            rejected.push((id.clone(), method.clone(), idempotent));
        }
    }
    if rejected.is_empty() {
        return Ok(());
    }
    pending_requests
        .lock()
        .await
        .retain(|(id, _, _)| !rejected.iter().any(|(rejected_id, _, _)| rejected_id == id));
    let mut stdout = editor_out.lock().await;
    for (id, method, idempotent) in rejected {
        let (code, message) = if idempotent {
            (
                -32801,
                format!(
                    "prod-code lsp: request '{method}' became stale during reconnect; retry against the updated document"
                ),
            )
        } else {
            (
                -32097,
                format!(
                    "prod-code lsp: request '{method}' interrupted by disconnect; non-idempotent operation was not retried to prevent duplicate side effects"
                ),
            )
        };
        let response =
            serde_json::json!({"jsonrpc":"2.0", "id":id, "error":{"code":code, "message":message}});
        prod_code_client::editor_files::write_frame(&mut *stdout, &response.to_string()).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_bridge::test_support::socket_pair;
    use futures_util::StreamExt;
    use std::sync::Arc;

    #[tokio::test]
    async fn cancellation_queued_during_initialization_is_not_replayed() {
        let (socket, mut peer) = socket_pair().await;
        let (mut tx, _rx) = socket.split();
        let temp = tempfile::tempdir().unwrap();
        let files = prod_code_client::editor_files::RemoteFiles::new(
            "127.0.0.1:1".parse().unwrap(),
            temp.path(),
            temp.path(),
            temp.path(),
        );
        let pending = Arc::new(tokio::sync::Mutex::new(vec![(
            serde_json::json!(7),
            "textDocument/hover".to_string(),
            serde_json::json!({"jsonrpc":"2.0","id":7,"method":"textDocument/hover"}).to_string(),
        )]));
        let deferred = VecDeque::from([serde_json::json!({
            "jsonrpc":"2.0", "method":"$/cancelRequest", "params":{"id":7}
        })
        .to_string()]);
        let stdout = tokio::sync::Mutex::new(tokio::io::stdout());
        replay_pending_requests(&mut tx, &files, &pending, &stdout, &deferred)
            .await
            .unwrap();
        assert!(pending.lock().await.is_empty());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), peer.next())
                .await
                .is_err()
        );
    }
}
