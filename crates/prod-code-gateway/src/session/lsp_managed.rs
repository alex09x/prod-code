/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::position::Flow;
use super::server_req::send_busy_note;
use super::shared_output::SharedOutputSender;
use crate::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

pub async fn handle_go_lsp(
    method: Option<&str>,
    id: &Option<serde_json::Value>,
    val: &serde_json::Value,
    view: &SessionView,
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
) -> Option<Flow> {
    let Some(ref go) = view.workspace.go_engine else {
        return None;
    };

    match method {
        Some("textDocument/didOpen") => {
            let uri = val
                .get("params")
                .and_then(|p| p.get("textDocument"))
                .and_then(|td| td.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let text = val
                .get("params")
                .and_then(|p| p.get("textDocument"))
                .and_then(|td| td.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let _ = go.did_open(uri, text).await;
            Some(Flow::Next)
        }
        Some("textDocument/didChange") => {
            let uri = val
                .get("params")
                .and_then(|p| p.get("textDocument"))
                .and_then(|td| td.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let version = val
                .get("params")
                .and_then(|p| p.get("textDocument"))
                .and_then(|td| td.get("version"))
                .and_then(|v| v.as_i64())
                .unwrap_or(1) as i32;
            let text = val
                .get("params")
                .and_then(|p| p.get("contentChanges"))
                .and_then(|c| c.as_array())
                .and_then(|a| a.first())
                .and_then(|ch| ch.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let _ = go.did_change(uri, text, version).await;
            Some(Flow::Next)
        }
        Some("textDocument/didClose") => {
            let uri = val
                .get("params")
                .and_then(|p| p.get("textDocument"))
                .and_then(|td| td.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let _ = go.did_close(uri).await;
            Some(Flow::Next)
        }
        Some(m) if id.is_some() => {
            let req_id_log = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
            let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
            TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
            let start = Instant::now();

            tracing::info!(
                req = req_id_log,
                session = view.session_id,
                method = m,
                in_flight,
                "🚀 [LSP START] dispatching to GoEngine"
            );

            let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
            let out_tx_task = out_tx.clone();
            let go_clone = Arc::clone(go);
            let translator_task = translator.clone();
            let session_id = view.session_id;
            let method_str = m.to_string();
            let req_id = id.clone();

            tokio::task::spawn(async move {
                let resp_res = go_clone.send_request(&method_str, params).await;
                let duration = start.elapsed();
                let duration_ms = duration.as_secs_f64() * 1000.0;
                let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

                if duration_ms > 200.0 {
                    SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(
                        req = req_id_log,
                        session = session_id,
                        method = %method_str,
                        duration_ms = %format!("{:.2}ms", duration_ms),
                        in_flight = remaining,
                        "⚠️ [LSP SLOW >200ms] GoEngine query exceeded threshold"
                    );
                } else {
                    tracing::info!(
                        req = req_id_log,
                        session = session_id,
                        method = %method_str,
                        duration_ms = %format!("{:.2}ms", duration_ms),
                        in_flight = remaining,
                        "✅ [LSP DONE] GoEngine query complete"
                    );
                }

                match resp_res {
                    Ok(mut resp) => {
                        send_busy_note(&mut resp, &out_tx_task).await;
                        if let Some(ref r_id) = req_id {
                            resp["id"] = r_id.clone();
                        }
                        let client_resp =
                            translator_task.translate_lsp_to_client(&resp.to_string());
                        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                    }
                    Err(err) => {
                        let err_resp = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": req_id,
                            "error": { "code": -32603, "message": err.to_string() }
                        });
                        let _ = out_tx_task
                            .send(WireMessage::LspPayload(err_resp.to_string()))
                            .await;
                    }
                }
            });
            Some(Flow::Next)
        }
        Some(m) => {
            let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
            let _ = go.send_notification(m, params).await;
            Some(Flow::Next)
        }
        None => None,
    }
}

pub async fn handle_generic_lsp(
    method: Option<&str>,
    id: &Option<serde_json::Value>,
    val: &serde_json::Value,
    view: &SessionView,
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
) -> Option<Flow> {
    let Some(ref generic_eng) = view.workspace.generic_engine else {
        return None;
    };

    if let (Some("textDocument/rename"), Some(req_id)) = (method, id) {
        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
        let engine = Arc::clone(generic_eng);
        let out_tx_task = out_tx.clone();
        let translator_task = translator.clone();
        let r_id = req_id.clone();
        let session_id = view.session_id;
        let start = Instant::now();
        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
        tokio::task::spawn(async move {
            let (resp, opened) = rename_with_references_open(&engine, params).await;
            tracing::info!(
                session = session_id,
                opened,
                duration_ms = format!("{:.2}ms", start.elapsed().as_secs_f64() * 1000.0),
                "✅ [LSP DONE] generic rename"
            );
            let resp = match resp {
                Ok(mut resp) => {
                    send_busy_note(&mut resp, &out_tx_task).await;
                    resp["id"] = r_id;
                    resp
                }
                Err(err) => {
                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "error": { "code": -32603, "message": err.to_string() } })
                }
            };
            let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
            let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
        });
        return Some(Flow::Next);
    }

    if let (Some("textDocument/diagnostic"), Some(req_id)) = (method, id) {
        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
        let engine = Arc::clone(generic_eng);
        let out_tx_task = out_tx.clone();
        let translator_task = translator.clone();
        let r_id = req_id.clone();
        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
        tokio::task::spawn(async move {
            let uri = params
                .get("textDocument")
                .and_then(|t| t.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string();
            let resp = match ManagedLsp::Generic(&engine).diagnostics_for(&uri).await {
                Ok(items) => {
                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "result": { "kind": "full", "items": items } })
                }
                Err(err) => {
                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "error": { "code": -32603, "message": err.to_string() } })
                }
            };
            let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
            let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
        });
        return Some(Flow::Next);
    }

    if let (Some(m), Some(req_id)) = (method, id) {
        let req_id_log = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
        let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
        let start = Instant::now();

        tracing::info!(
            req = req_id_log,
            session = view.session_id,
            method = m,
            in_flight,
            "🚀 [LSP START] dispatching to GenericLspEngine"
        );

        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
        let out_tx_task = out_tx.clone();
        let generic_eng_clone = Arc::clone(generic_eng);
        let translator_task = translator.clone();
        let session_id = view.session_id;
        let method_str = m.to_string();
        let r_id = req_id.clone();

        tokio::task::spawn(async move {
            let resp_res = generic_eng_clone.send_request(&method_str, params).await;
            let duration = start.elapsed();
            let duration_ms = duration.as_secs_f64() * 1000.0;
            let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

            if duration_ms > 200.0 {
                SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    req = req_id_log,
                    session = session_id,
                    method = %method_str,
                    duration_ms = %format!("{:.2}ms", duration_ms),
                    in_flight = remaining,
                    "⚠️ [LSP SLOW >200ms] GenericLspEngine query exceeded threshold"
                );
            } else {
                tracing::info!(
                    req = req_id_log,
                    session = session_id,
                    method = %method_str,
                    duration_ms = %format!("{:.2}ms", duration_ms),
                    in_flight = remaining,
                    "✅ [LSP DONE] GenericLspEngine query complete"
                );
            }

            match resp_res {
                Ok(mut resp) => {
                    send_busy_note(&mut resp, &out_tx_task).await;
                    resp["id"] = r_id;
                    let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                    let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                }
                Err(err) => {
                    let err_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": r_id,
                        "error": { "code": -32603, "message": err.to_string() }
                    });
                    let _ = out_tx_task
                        .send(WireMessage::LspPayload(err_resp.to_string()))
                        .await;
                }
            }
        });
        return Some(Flow::Next);
    } else if let Some(m) = method {
        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
        let _ = generic_eng
            .send_session_notification(view.session_id, m, params)
            .await;
        return Some(Flow::Next);
    }

    None
}

pub async fn forward_to_backend(server_lsp: &str, view: &SessionView) {
    if let Some(backend) = &view.workspace.backend {
        let _ = backend.send_lsp(server_lsp).await.inspect_err(|e| {
            tracing::error!(error = %e, "Failed to forward LSP to backend worker");
        });
    }
}
