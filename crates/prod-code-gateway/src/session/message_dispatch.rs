/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::lsp_intercept::*;
use super::lsp_managed::*;
use super::lsp_rust::*;
use super::position::*;
use super::shared_output::*;
use super::sync_handler::*;
use crate::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

pub async fn on_client_message(
    client_msg_res: Option<std::result::Result<WireMessage, std::io::Error>>,
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    meta: &Arc<SessionMeta>,
    pending: &Arc<tokio::sync::Mutex<std::collections::HashMap<String, PendingRequest>>>,
) -> Flow {
    match client_msg_res {
        Some(Ok(WireMessage::Ping)) => {
            let _ = out_tx.send(WireMessage::Pong).await;
        }
        Some(Ok(WireMessage::LspPayload(raw_client_lsp))) => {
            let server_lsp = translator.translate_lsp_to_server(&raw_client_lsp);
            tracing::debug!(
                payload_len = server_lsp.len(),
                single_owner = view.is_single_owner(),
                "Processing incoming LSP message"
            );

            // Inspect LSP message structure
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&server_lsp) {
                let method = val.get("method").and_then(|m| m.as_str());
                let id = val.get("id").cloned();
                if view.workspace.rust_engine.is_some()
                    && let Err(reason) = native_position_params(method, val.get("params"))
                {
                    if let (Some(method), Some(id)) =
                        (method, id.as_ref().filter(|id| !id.is_null()))
                    {
                        send_invalid_params(out_tx, translator, id, method, &reason).await;
                    }
                    return Flow::Next;
                }
                if let (Some(m), Some(id_val)) = (method, &id)
                    && !id_val.is_null()
                    && m != "initialize"
                {
                    let params = val.get("params");
                    let uri = params
                        .and_then(|p| {
                            p.get("textDocument")
                                .and_then(|t| t.get("uri"))
                                .or_else(|| p.get("item").and_then(|i| i.get("uri")))
                        })
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    let path = uri_or_path(uri);
                    let file = path
                        .strip_prefix(&meta.engine_root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned();
                    let pos = params.and_then(|p| {
                        p.get("position")
                            .or_else(|| p.get("range").and_then(|r| r.get("start")))
                            .or_else(|| {
                                p.get("item")
                                    .and_then(|item| item.get("selectionRange"))
                                    .and_then(|range| range.get("start"))
                            })
                    });
                    let (line, col) = metric_position(pos);
                    pending.lock().await.insert(
                        id_val.to_string(),
                        PendingRequest {
                            method: m.to_string(),
                            file,
                            line,
                            col,
                            start: Instant::now(),
                        },
                    );
                }

                // 1-3. Intercept lifecycle: initialize, initialized, shutdown
                if let Some(flow) = intercept_lifecycle_lsp(method, &id, view, out_tx, translator).await {
                    return flow;
                }

                // 4. In-Memory RustEngine multi-core fast path
                if let Some(ref engine_lock) = view.workspace.rust_engine {
                    if let Some(flow) = handle_rust_lsp(
                        out_tx,
                        translator,
                        view,
                        meta,
                        method,
                        &id,
                        &val,
                        engine_lock,
                    )
                    .await
                    {
                        return flow;
                    }
                }

                // 5. Backend worker didOpen vs didChange / didClose fallback
                if let Some(flow) = intercept_backend_open_or_close(method, &val, view).await {
                    return flow;
                }

                // 5a'. Managed assists
                if let Some(flow) = intercept_assists_for_managed(method, &id, &val, view, out_tx, translator) {
                    return flow;
                }

                // 5b. GoEngine fast path
                if let Some(flow) = handle_go_lsp(method, &id, &val, view, out_tx, translator).await {
                    return flow;
                }

                // 5c. GenericLspEngine fast path
                if let Some(flow) = handle_generic_lsp(method, &id, &val, view, out_tx, translator).await {
                    return flow;
                }

                // 7. Fallback empty LSP
                if let Some(flow) = fallback_empty_lsp(&id, view, out_tx).await {
                    return flow;
                }
            }

            forward_to_backend(&server_lsp, view).await;
        }
        Some(Ok(WireMessage::SyncRequest(req))) => {
            handle_session_sync(req, view, out_tx).await;
        }
        Some(Ok(WireMessage::Disconnect { reason })) => {
            tracing::info!(reason, "Client terminated session");
            return Flow::Stop;
        }
        Some(Ok(WireMessage::StatusRequest)) => {
            let _ = out_tx
                .send(WireMessage::StatusResponse(StatusResponse {
                    server_pid: std::process::id(),
                    uptime_seconds: 0,
                    active_sessions: 1,
                    loaded_workspaces: 1,
                    detected_engines: vec![view.workspace.engine.clone()],
                    memory_rss_bytes: memory::get_process_rss_bytes(),
                    total_queries: TOTAL_QUERIES.load(Ordering::Relaxed),
                    active_queries: ACTIVE_QUERIES.load(Ordering::Relaxed),
                    load_average_millis: memory::load_average_1m().map(|l| (l * 1000.0) as u32),
                    cpu_count: std::thread::available_parallelism().ok().map(|n| n.get()),
                    platform: Some(prod_code_protocol::platform()),
                    running_commands: running_commands(),
                    host: memory::host_resources(&view.worktree_root),
                    version: Some(env!("CARGO_PKG_VERSION").to_string()),
                    git_commit: Some(prod_code_protocol::git_commit().to_string())
                        .filter(|c| c != "unknown"),
                }))
                .await;
        }
        Some(Ok(WireMessage::ReadFileRequest(req))) => {
            let resp = read_server_file(&meta.storage_root, &req);
            let _ = out_tx.send(WireMessage::ReadFileResponse(resp)).await;
        }
        Some(Err(e)) => {
            tracing::error!(error = %e, "TCP frame decode error");
            return Flow::Stop;
        }
        None => {
            tracing::debug!("Client disconnected");
            return Flow::Stop;
        }
        _ => {}
    }
    Flow::Next
}
