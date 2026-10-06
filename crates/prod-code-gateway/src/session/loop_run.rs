/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::message_dispatch::on_client_message;
use super::position::Flow;
use super::server_req::{fallback_answers_request, is_server_request};
use super::shared_output::*;
use crate::*;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;

pub async fn run_session_loop(
    framed: Framed<AnyStream, ProdCodeCodec>,
    translator: &PathTranslator,
    view: &SessionView,
    meta: Arc<SessionMeta>,
) -> Result<()> {
    let (mut socket_tx, mut socket_rx) = framed.split();
    // rapidfire MPSC: every engine task sends, one writer drains in batches and flushes the
    // socket once per batch.
    let (raw_out_tx, mut out_rx) =
        rapidfire::mpsc::bounded::<SharedOutputFrame>(SHARED_OUTPUT_CAPACITY);
    let out_tx = SharedOutputSender::new(raw_out_tx, SHARED_OUTPUT_WRITE_BUDGET);

    // Requests in flight, keyed by JSON-RPC id, so every answer — whichever engine produced
    // it — becomes one metrics event with its duration.
    let pending: Arc<tokio::sync::Mutex<std::collections::HashMap<String, PendingRequest>>> =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let pending_writer = Arc::clone(&pending);
    let meta_writer = Arc::clone(&meta);
    let writer_output = out_tx.clone();
    let writer_handle = tokio::spawn(async move {
        let _lifetime = SharedWriterLifetime::start(writer_output);
        let mut batch: Vec<SharedOutputFrame> = Vec::with_capacity(SHARED_OUTPUT_BATCH);
        while out_rx
            .recv_many(&mut batch, SHARED_OUTPUT_BATCH)
            .await
            .is_ok()
        {
            let mut flush_deadline = None;
            for frame in batch.drain(..) {
                if tokio::time::Instant::now() >= frame.deadline {
                    anyhow::bail!("shared output frame expired while queued");
                }
                if let WireMessage::LspPayload(ref raw) = frame.message
                    && let Ok(val) = serde_json::from_str::<serde_json::Value>(raw)
                    && let Some(id) = val.get("id").filter(|i| !i.is_null())
                    && val.get("method").is_none()
                {
                    let key = id.to_string();
                    if let Some(req) = pending_writer.lock().await.remove(&key) {
                        let mut ev = metrics::Event::blank("lsp");
                        ev.session_id = meta_writer.session_id;
                        ev.client_name = meta_writer.client_name.clone();
                        ev.agent = meta_writer.agent.clone();
                        ev.host = meta_writer.host.clone();
                        ev.client_addr = meta_writer.client_addr.clone();
                        ev.workspace = meta_writer.workspace.clone();
                        ev.engine = meta_writer.engine.clone();
                        ev.method = req.method;
                        ev.file = req.file;
                        ev.line = req.line;
                        ev.col = req.col;
                        ev.duration_ms = req.start.elapsed().as_millis() as u64;
                        ev.ok = val.get("error").is_none();
                        ev.items = val
                            .get("result")
                            .map(|r| match r {
                                serde_json::Value::Array(a) => a.len() as u64,
                                serde_json::Value::Null => 0,
                                _ => 1,
                            })
                            .unwrap_or(0);
                        meta_writer.metrics.record(ev);
                    }
                }
                tokio::time::timeout_at(frame.deadline, socket_tx.feed(frame.message))
                    .await
                    .map_err(|_| anyhow::anyhow!("shared socket feed deadline elapsed"))??;
                flush_deadline = Some(match flush_deadline {
                    Some(current) => std::cmp::min(current, frame.deadline),
                    None => frame.deadline,
                });
            }
            if let Some(deadline) = flush_deadline {
                tokio::time::timeout_at(deadline, socket_tx.flush())
                    .await
                    .map_err(|_| anyhow::anyhow!("shared socket flush deadline elapsed"))??;
            }
        }
        Ok(())
    });
    // Install abort ownership before the session can reach another await. Cancellation of the
    // handler must cancel this exact writer, whose lifetime guard closes every producer queue.
    let mut writer = OwnedJoin::new(writer_handle);

    // gopls and the supervised servers answer their own requests (`window/workDoneProgress/create`,
    // `workspace/configuration`) in the engine; passed on, one carried the id of a client's
    // question and was taken for its answer (#391). Only their notifications go to the client.
    let engine_answers_requests =
        view.workspace.go_engine.is_some() || view.workspace.generic_engine.is_some();
    let mut backend_rx = if let Some(ref go) = view.workspace.go_engine {
        Some(go.subscribe())
    } else if let Some(ref generic_eng) = view.workspace.generic_engine {
        Some(generic_eng.subscribe())
    } else {
        view.workspace.backend.as_ref().map(|b| b.subscribe())
    };

    let mut rebalance_rx = view.accounted.subscribe_rebalance();
    let mut writer_finished = false;
    let mut session_result = Ok(());
    loop {
        tokio::select! {
            writer_result = writer.task_mut() => {
                writer.clear_finished();
                writer_finished = true;
                session_result = flatten_writer_result(writer_result);
                break;
            }
            client_msg_res = socket_rx.next() => {
                match on_client_message(client_msg_res, &out_tx, translator, view, &meta, &pending).await {
                    Flow::Next => continue,
                    Flow::Stop => break,
                }
            }

            rebalance_msg = rebalance_rx.recv() => {
                match rebalance_msg {
                    Ok((target_addr, reason)) => {
                        tracing::info!(
                            session_id = meta.session_id,
                            target = %target_addr,
                            ?reason,
                            "Session rebalanced: sending Redirect frame to active client"
                        );
                        let _ = out_tx.send(WireMessage::Redirect { target_addr, reason }).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
                }
            }

            backend_msg = async {
                if let Some(ref mut rx) = backend_rx {
                    rx.recv().await
                } else {
                    futures_util::future::pending::<Result<String, tokio::sync::broadcast::error::RecvError>>().await
                }
            } => {
                match backend_msg {
                    Ok(server_lsp) => {
                        if (engine_answers_requests && is_server_request(&server_lsp))
                            || (!engine_answers_requests && fallback_answers_request(&server_lsp))
                        {
                            continue;
                        }
                        let client_lsp = translator.translate_lsp_to_client(&server_lsp);
                        if out_tx.send(WireMessage::LspPayload(client_lsp)).await.is_err() {
                            tracing::error!("Failed to send LSP message to client channel");
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "Session backend receiver lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::warn!("Backend worker broadcast closed");
                        break;
                    }
                }
            }
        }
    }
    out_tx.close();
    if !writer_finished {
        let teardown_deadline = tokio::time::Instant::now() + SHARED_OUTPUT_TEARDOWN_BUDGET;
        match tokio::time::timeout_at(teardown_deadline, writer.task_mut()).await {
            Ok(writer_result) => {
                writer.clear_finished();
                let writer_result = flatten_writer_result(writer_result);
                if session_result.is_ok() {
                    session_result = writer_result;
                }
            }
            Err(_) => {
                writer.abort();
                // Once aborted, await the exact task so no writer is detached. This is cleanup
                // after the single teardown deadline, not a second drain budget.
                let writer_result = writer.task_mut().await;
                writer.clear_finished();
                if let Err(error) = writer_result
                    && !error.is_cancelled()
                {
                    tracing::warn!(%error, "shared output writer failed while being aborted");
                }
                if session_result.is_ok() {
                    session_result = Err(anyhow::anyhow!(
                        "shared output writer exceeded teardown budget"
                    ));
                }
            }
        }
    }
    session_result
}
