/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::transport::{
    EditorFrameReceiver, MAX_DEFERRED_EDITOR_BYTES, MAX_DEFERRED_EDITOR_FRAMES, PendingRequests,
};
use anyhow::Result;
use std::collections::VecDeque;
use std::future::Future;

#[derive(Debug)]
pub(super) struct EditorStopped {
    pub clean: bool,
    pub reason: String,
}

impl std::fmt::Display for EditorStopped {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for EditorStopped {}

#[derive(Debug)]
pub(super) struct IncompatiblePositionEncoding;

impl std::fmt::Display for IncompatiblePositionEncoding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("gateway changed LSP position encoding; restart the language server")
    }
}

impl std::error::Error for IncompatiblePositionEncoding {}

pub(super) fn defer_editor_message(deferred: &mut VecDeque<String>, json: String) -> Result<()> {
    let bytes = deferred
        .iter()
        .fold(0_usize, |sum, item| sum.saturating_add(item.len()));
    if deferred.len() >= MAX_DEFERRED_EDITOR_FRAMES
        || bytes.saturating_add(json.len()) > MAX_DEFERRED_EDITOR_BYTES
    {
        return Err(EditorStopped {
            clean: false,
            reason: "too many editor messages queued during language-server reconnect".to_string(),
        }
        .into());
    }
    deferred.push_back(json);
    Ok(())
}

pub(super) async fn shutdown_during_reconnect<T>(
    id: &serde_json::Value,
    frames: &EditorFrameReceiver,
    editor_out: &tokio::sync::Mutex<tokio::io::Stdout>,
) -> Result<T> {
    let reply = serde_json::json!({"jsonrpc":"2.0", "id":id, "result":null});
    let mut output = editor_out.lock().await;
    if let Err(err) =
        prod_code_client::editor_files::write_frame(&mut *output, &reply.to_string()).await
    {
        return Err(EditorStopped {
            clean: false,
            reason: format!("replying to editor shutdown: {err}"),
        }
        .into());
    }
    drop(output);
    loop {
        match frames.lock().await.recv().await {
            Some(Ok(Some(json))) => {
                if prod_code_client::editor_files::method_of(&json).as_deref() == Some("exit") {
                    break;
                }
            }
            Some(Ok(None)) | None => break,
            Some(Err(err)) => {
                return Err(EditorStopped {
                    clean: false,
                    reason: format!("reading editor message during reconnect: {err}"),
                }
                .into());
            }
        }
    }
    Err(EditorStopped {
        clean: true,
        reason: "editor exited during language-server reconnect".to_string(),
    }
    .into())
}

/// Keep accepting editor messages while the gateway is unavailable. Requests and
/// notifications retain their original order and are forwarded after state replay.
pub(super) async fn while_reconnecting<F, T>(
    future: F,
    frames: &EditorFrameReceiver,
    deferred: &mut VecDeque<String>,
    pending: &PendingRequests,
    editor_out: &tokio::sync::Mutex<tokio::io::Stdout>,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return result,
            frame = async { frames.lock().await.recv().await } => {
                let json = match frame {
                    Some(Ok(Some(json))) => json,
                    Some(Ok(None)) | None => return Err(EditorStopped {
                        clean: false,
                        reason: "editor closed during language-server reconnect".to_string(),
                    }.into()),
                    Some(Err(err)) => return Err(EditorStopped {
                        clean: false,
                        reason: format!("reading editor message during reconnect: {err}"),
                    }.into()),
                };
                let value = serde_json::from_str::<serde_json::Value>(&json).ok();
                let method = value.as_ref().and_then(|value| value.get("method")).and_then(|method| method.as_str());
                if method == Some("shutdown") && let Some(id) = value.as_ref().and_then(|value| value.get("id")) {
                    return shutdown_during_reconnect(id, frames, editor_out).await;
                }
                if method == Some("exit") {
                    return Err(EditorStopped {
                        clean: false,
                        reason: "editor exited during language-server reconnect".to_string(),
                    }.into());
                }
                if method == Some("$/cancelRequest")
                    && let Some(id) = value.as_ref().and_then(|value| value.pointer("/params/id"))
                {
                    pending.lock().await.retain(|(pending_id, _, _)| pending_id != id);
                }
                defer_editor_message(deferred, json)?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn deferred_messages_are_bounded_by_count_and_bytes() {
        let mut by_count = VecDeque::new();
        for _ in 0..MAX_DEFERRED_EDITOR_FRAMES {
            defer_editor_message(&mut by_count, String::new()).unwrap();
        }
        assert!(defer_editor_message(&mut by_count, String::new()).is_err());

        let mut by_bytes = VecDeque::new();
        assert!(
            defer_editor_message(&mut by_bytes, "x".repeat(MAX_DEFERRED_EDITOR_BYTES + 1)).is_err()
        );
        assert!(by_bytes.is_empty());
    }

    #[tokio::test]
    async fn editor_eof_interrupts_a_stalled_connection_attempt() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        sender.send(Ok(None)).await.unwrap();
        let frames = Arc::new(tokio::sync::Mutex::new(receiver));
        let pending = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let output = tokio::sync::Mutex::new(tokio::io::stdout());
        let mut deferred = VecDeque::new();
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            while_reconnecting(
                std::future::pending::<Result<()>>(),
                &frames,
                &mut deferred,
                &pending,
                &output,
            ),
        )
        .await
        .expect("editor EOF must interrupt the stalled attempt");
        let error = outcome.unwrap_err();
        assert!(error.is::<EditorStopped>());
        assert!(!error.downcast_ref::<EditorStopped>().unwrap().clean);
    }
}
