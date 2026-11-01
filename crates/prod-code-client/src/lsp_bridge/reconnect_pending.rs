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
use futures_util::stream::SplitSink;
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use std::collections::VecDeque;
use tokio_util::codec::Framed;

pub(super) fn replay_request(method: &str, request: &str) -> Option<String> {
    if method.ends_with("/resolve")
        || matches!(
            method,
            "callHierarchy/incomingCalls"
                | "callHierarchy/outgoingCalls"
                | "typeHierarchy/supertypes"
                | "typeHierarchy/subtypes"
        )
    {
        return None;
    }
    if method == "textDocument/diagnostic" || method == "workspace/diagnostic" {
        let mut value: serde_json::Value = serde_json::from_str(request).ok()?;
        let params = value.get_mut("params")?.as_object_mut()?;
        if method == "textDocument/diagnostic" {
            params.remove("previousResultId");
        } else {
            params.insert("previousResultIds".into(), serde_json::json!([]));
        }
        return Some(value.to_string());
    }
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
    _socket: &mut SplitSink<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>, WireMessage>,
    _files: &prod_code_client::editor_files::RemoteFiles,
    pending_requests: &PendingRequests,
    editor_out: &tokio::sync::Mutex<tokio::io::Stdout>,
    deferred: &VecDeque<String>,
) -> Result<()> {
    // An in-flight request may refer to an older document or server session.
    // Fail it so the editor can retry against the restored state.
    let cancelled: Vec<serde_json::Value> = deferred
        .iter()
        .filter_map(|frame| {
            let message: serde_json::Value = serde_json::from_str(frame).ok()?;
            (message.get("method")?.as_str()? == "$/cancelRequest")
                .then(|| message.pointer("/params/id").cloned())
                .flatten()
        })
        .collect();
    let pending = std::mem::take(&mut *pending_requests.lock().await);
    if pending.is_empty() {
        return Ok(());
    }
    let mut stdout = editor_out.lock().await;
    for (id, method, _) in pending {
        let (code, message) = if method == "$/cancelRequest" || cancelled.contains(&id) {
            (-32800, "request cancelled".to_string())
        } else if is_idempotent_lsp_request(&method) {
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

    #[test]
    fn old_session_resolve_requests_are_not_replayed() {
        for method in [
            "completionItem/resolve",
            "codeLens/resolve",
            "documentLink/resolve",
            "inlayHint/resolve",
            "workspace/symbol/resolve",
            "callHierarchy/incomingCalls",
            "callHierarchy/outgoingCalls",
            "typeHierarchy/supertypes",
            "typeHierarchy/subtypes",
        ] {
            assert!(
                replay_request(method, r#"{"jsonrpc":"2.0","id":1}"#).is_none(),
                "{method}"
            );
        }
    }

    #[test]
    fn replayed_diagnostics_do_not_reuse_old_result_ids() {
        for (method, key) in [
            ("textDocument/diagnostic", "previousResultId"),
            ("workspace/diagnostic", "previousResultIds"),
        ] {
            let request = serde_json::json!({"jsonrpc":"2.0", "id":1, "method":method,
                "params": {(key): "old-session-token"}})
            .to_string();
            let replayed: serde_json::Value =
                serde_json::from_str(&replay_request(method, &request).unwrap()).unwrap();
            if method == "workspace/diagnostic" {
                assert_eq!(replayed["params"][key], serde_json::json!([]));
            } else {
                assert!(replayed["params"].get(key).is_none(), "{method}");
            }
        }
    }

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

    #[tokio::test]
    async fn request_superseded_by_an_edit_before_disconnect_is_not_replayed() {
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
        replay_pending_requests(
            &mut tx,
            &files,
            &pending,
            &tokio::sync::Mutex::new(tokio::io::stdout()),
            &VecDeque::new(),
        )
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
