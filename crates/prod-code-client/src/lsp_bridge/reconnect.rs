/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::encoding::update_server_capabilities;
use super::reconnect_pending::replay_pending_requests;
use super::reconnect_wait::{
    EditorExit, EditorShutdown, EditorStopped, IncompatiblePositionEncoding, ReconnectTimeout,
    defer_editor_message, while_reconnecting,
};
pub use super::reconnect_workspace::replay_lsp_workspace_state;
use super::session::open_editor_session;
use super::state::LspStateTracker;
use super::transport::{
    EditorFrameReceiver, LspTrace, PendingRequests, trace_message, trace_reconnect,
};
use anyhow::{Context, Result};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::AtomicU8;
use tokio_util::codec::Framed;

pub(super) fn should_reconnect_after_close(
    reconnect_enabled: bool,
    is_redirect: bool,
    reason: &str,
) -> bool {
    reconnect_enabled
        || is_redirect
        || reason.ends_with(prod_code_protocol::WORKSPACE_MODEL_CHANGED_REASON)
}

#[cfg(test)]
#[path = "reconnect_capability_tests.rs"]
mod capability_tests;
#[cfg(test)]
#[path = "reconnect_tests.rs"]
mod tests;

pub async fn replay_lsp_state(
    socket_tx: &mut SplitSink<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>, WireMessage>,
    socket_rx: &mut SplitStream<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>>,
    tracker: &LspStateTracker,
    expect_original_encoding: bool,
    files: &prod_code_client::editor_files::RemoteFiles,
    position_encoding: &AtomicU8,
    editor_frames: &EditorFrameReceiver,
    deferred_editor_frames: &mut std::collections::VecDeque<String>,
    pending_requests: &PendingRequests,
    editor_out: &tokio::sync::Mutex<tokio::io::Stdout>,
    trace: &LspTrace,
) -> Result<Option<String>> {
    let mut replayed_initialize = None;
    if let Some(ref init_req) = tracker.initialize_req {
        let initialize_id = serde_json::from_str::<serde_json::Value>(init_req)
            .ok()
            .and_then(|value| value.get("id").cloned())
            .context("recorded initialize request has no JSON-RPC id")?;
        socket_tx
            .send(WireMessage::LspPayload(files.to_node(init_req)))
            .await
            .context("failed to send replayed initialize request")?;

        let init_timeout = tokio::time::Duration::from_secs(10);
        let mut server_request_ids: Vec<serde_json::Value> = Vec::new();
        let mut initialize_response: Option<String> = None;

        tokio::time::timeout(init_timeout, async {
            loop {
                tokio::select! {
                    message = socket_rx.next() => match message {
                        Some(Ok(WireMessage::LspPayload(json))) => {
                            let parsed = serde_json::from_str::<serde_json::Value>(&json).ok();
                            if let Some(value) = &parsed {
                                if value.get("id") == Some(&initialize_id) && value.get("method").is_none() {
                                    if value.get("error").is_some() || !value.get("result").is_some_and(serde_json::Value::is_object) {
                                        anyhow::bail!("server rejected the replayed initialize request");
                                    }
                                    if !update_server_capabilities(position_encoding, value, expect_original_encoding) {
                                        return Err(IncompatiblePositionEncoding.into());
                                    }
                                    initialize_response = Some(json);
                                    if server_request_ids.is_empty() {
                                        return Ok(());
                                    }
                                    continue;
                                }
                                if value.get("method").is_some() {
                                    if let Some(id) = value.get("id") {
                                        server_request_ids.push(id.clone());
                                    }
                                }
                            }
                            let editor_json = files.to_editor(json).await;
                            trace_message(trace, "<-", &editor_json);
                            let mut stdout = editor_out.lock().await;
                            prod_code_client::editor_files::write_frame(&mut *stdout, &editor_json)
                                .await
                                .context("forwarding server initialization message to the editor")?;
                        }
                        Some(Ok(WireMessage::Pong)) => {}
                        Some(Ok(other)) => anyhow::bail!("unexpected message during initialize replay: {other:?}"),
                        Some(Err(err)) => anyhow::bail!("error during initialize replay: {err}"),
                        None => anyhow::bail!("connection closed while awaiting initialize replay response"),
                    },
                    frame = async { editor_frames.lock().await.recv().await } => match frame {
                        Some(Ok(Some(json))) => {
                            let value = serde_json::from_str::<serde_json::Value>(&json).ok();
                            if value.as_ref().and_then(|value| value.get("method")).and_then(|method| method.as_str()) == Some("exit") {
                                return Err(EditorExit.into());
                            }
                            if value.as_ref().and_then(|value| value.get("method")).and_then(|method| method.as_str()) == Some("shutdown")
                                && let Some(id) = value.as_ref().and_then(|value| value.get("id"))
                            {
                                return Err(EditorShutdown(id.clone()).into());
                            }
                            let id = value.as_ref().and_then(|value| {
                                    (value.get("method").is_none()).then(|| value.get("id").cloned()).flatten()
                                });
                            if let Some(id) = id {
                                if let Some(index) = server_request_ids.iter().position(|pending| *pending == id) {
                                    server_request_ids.remove(index);
                                    socket_tx.send(WireMessage::LspPayload(files.to_node(&json)))
                                        .await
                                        .context("forwarding editor response to server initialization request")?;
                                    if initialize_response.is_some() && server_request_ids.is_empty() {
                                        return Ok(());
                                    }
                                    continue;
                                }
                            }
                            defer_editor_message(deferred_editor_frames, json)?;
                        }
                        Some(Ok(None)) | None => return Err(EditorStopped {
                            clean: false,
                            reason: "editor closed during language-server reconnect".to_string(),
                        }.into()),
                        Some(Err(err)) => return Err(EditorStopped {
                            clean: false,
                            reason: format!("reading editor message during reconnect: {err}"),
                        }.into()),
                    }
                }
            }
        })
        .await
        .context("timed out waiting for initialize replay response")??;
        replayed_initialize = Some(
            initialize_response
                .context("initialize replay completed without a matching response")?,
        );
    }

    if tracker.initialized_sent {
        let initialized = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        });
        let replay = async {
            socket_tx
                .send(WireMessage::LspPayload(initialized.to_string()))
                .await
                .context("failed to send replayed initialized notification")?;
            replay_lsp_workspace_state(socket_tx, tracker, files).await
        };
        while_reconnecting(
            replay,
            editor_frames,
            deferred_editor_frames,
            pending_requests,
            editor_out,
        )
        .await?;
    }

    Ok(replayed_initialize)
}

pub async fn reconnect_editor_session(
    remote: SocketAddr,
    is_redirect: bool,
    engine: Option<&str>,
    cwd: &Path,
    cwd_str: &str,
    identity: &prod_code_mcp::sync::WorkspaceIdentity,
    tracker: &LspStateTracker,
    files: &prod_code_client::editor_files::RemoteFiles,
    pending_requests: &PendingRequests,
    editor_out: &tokio::sync::Mutex<tokio::io::Stdout>,
    position_encoding: &AtomicU8,
    editor_frames: &EditorFrameReceiver,
    deferred_editor_frames: &mut std::collections::VecDeque<String>,
    trace: &LspTrace,
) -> Result<(
    SplitSink<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>, WireMessage>,
    SplitStream<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>>,
    bool,
    SocketAddr,
)> {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(120);
    let mut backoff = tokio::time::Duration::from_millis(200);
    let mut attempt = 0_u32;
    let mut last_err = None;
    let initial_redirect_count = if is_redirect { 1 } else { 0 };
    while tokio::time::Instant::now() < deadline {
        if attempt > 0 {
            let delay =
                tokio::time::sleep_until((tokio::time::Instant::now() + backoff).min(deadline));
            while_reconnecting(
                async {
                    delay.await;
                    Ok(())
                },
                editor_frames,
                deferred_editor_frames,
                pending_requests,
                editor_out,
            )
            .await?;
            backoff = (backoff * 2).min(tokio::time::Duration::from_secs(3));
            if tokio::time::Instant::now() >= deadline {
                break;
            }
        }
        attempt += 1;
        trace_reconnect(trace, "attempt", attempt);
        let attempt_deadline =
            (tokio::time::Instant::now() + tokio::time::Duration::from_secs(30)).min(deadline);
        let open = async {
            tokio::time::timeout_at(
                attempt_deadline,
                open_editor_session(
                    remote,
                    engine,
                    cwd,
                    cwd_str.to_string(),
                    identity.clone(),
                    initial_redirect_count,
                ),
            )
            .await
            .context("timed out opening an editor session")?
        };
        let opened = while_reconnecting(
            open,
            editor_frames,
            deferred_editor_frames,
            pending_requests,
            editor_out,
        )
        .await;
        if opened.as_ref().err().is_some_and(|err| {
            err.is::<EditorStopped>() || err.is::<EditorExit>() || err.is::<EditorShutdown>()
        }) {
            return Err(opened.err().expect("checked for an editor stop"));
        }
        match opened {
            Ok((framed, handshake_resp, effective_remote)) => {
                let replay_deadline = (tokio::time::Instant::now()
                    + tokio::time::Duration::from_secs(30))
                .min(deadline);
                files.set_node(
                    effective_remote,
                    Path::new(&handshake_resp.server_workspace_root),
                );
                let (mut new_tx, mut new_rx) = framed.split();
                let pending_initialize_id = {
                    let pending = pending_requests.lock().await;
                    pending
                        .iter()
                        .find(|(_, method, _)| method == "initialize")
                        .map(|(id, _, _)| id.clone())
                };
                let replay = tokio::time::timeout_at(
                    replay_deadline,
                    replay_lsp_state(
                        &mut new_tx,
                        &mut new_rx,
                        tracker,
                        pending_initialize_id.is_none(),
                        files,
                        position_encoding,
                        editor_frames,
                        deferred_editor_frames,
                        pending_requests,
                        editor_out,
                        trace,
                    ),
                )
                .await;
                let replay = match replay {
                    Ok(result) => result,
                    Err(_) => {
                        trace_reconnect(trace, "replay_timeout", attempt);
                        return Err(ReconnectTimeout("timed out restoring editor state").into());
                    }
                };
                match replay {
                    Ok(replayed_initialize) => {
                        if let Some(initialize_id) = pending_initialize_id {
                            let Some(response) = replayed_initialize else {
                                last_err = Some(anyhow::anyhow!(
                                    "replayed initialize produced no response for the editor's pending request"
                                ));
                                continue;
                            };
                            let response = files.to_editor(response).await;
                            let mut stdout = editor_out.lock().await;
                            if let Err(err) =
                                prod_code_client::editor_files::write_frame(&mut *stdout, &response)
                                    .await
                            {
                                last_err = Some(err.into());
                                continue;
                            }
                            drop(stdout);
                            pending_requests.lock().await.retain(|(id, method, _)| {
                                id != &initialize_id || method != "initialize"
                            });
                        }
                        tokio::time::timeout_at(
                            replay_deadline,
                            replay_pending_requests(
                                &mut new_tx,
                                files,
                                pending_requests,
                                editor_out,
                                deferred_editor_frames,
                            ),
                        )
                        .await
                        .map_err(|_| {
                            ReconnectTimeout("timed out answering interrupted requests")
                        })??;

                        tokio::time::timeout_at(
                            replay_deadline,
                            super::reconnect_deferred::prepare_deferred(
                                deferred_editor_frames,
                                editor_out,
                            ),
                        )
                        .await
                        .map_err(|_| ReconnectTimeout("timed out preparing queued requests"))??;
                        tracing::info!(
                            "successfully reconnected to gateway and replayed LSP state"
                        );
                        trace_reconnect(trace, "restored", attempt);
                        return Ok((new_tx, new_rx, !tracker.initialized_sent, effective_remote));
                    }
                    Err(err) => {
                        if err.is::<EditorStopped>()
                            || err.is::<EditorExit>()
                            || err.is::<EditorShutdown>()
                            || err.is::<IncompatiblePositionEncoding>()
                        {
                            return Err(err);
                        }
                        tracing::warn!(%err, "LSP state replay failed during reconnect attempt");
                        trace_reconnect(trace, "replay_failed", attempt);
                        last_err = Some(err);
                    }
                }
            }
            Err(err) => {
                tracing::warn!(%err, "reconnect attempt failed to open editor session");
                trace_reconnect(trace, "attempt_failed", attempt);
                last_err = Some(err);
            }
        }
    }

    trace_reconnect(trace, "deadline_exhausted", attempt);
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("all reconnect attempts failed")))
}
