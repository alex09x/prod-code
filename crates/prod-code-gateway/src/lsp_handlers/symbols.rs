/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

pub(crate) fn lsp_workspace_symbol(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let query = params
        .get("query")
        .and_then(|q| q.as_str())
        .unwrap_or("")
        .to_string();
    let limit = params.get("limit").and_then(|l| l.as_u64()).unwrap_or(64) as usize;
    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(req = req_num, session = view.session_id, method = "workspace/symbol", query = %query, in_flight, "🚀 [LSP START]");

    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let ws_root = view.workspace.root.clone();
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let q = query.clone();
        let syms = execute_bounded_query(
            &engine_arc,
            session_id,
            &ws_root,
            is_single_owner,
            move |snapshot| snapshot.workspace_symbols(&q, limit),
        )
        .await
        .unwrap_or_default();
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        tracing::info!(
            req = req_num,
            session = session_id,
            method = "workspace/symbol",
            duration_ms = format!("{:.2}ms", ms),
            symbols = syms.len(),
            in_flight = remaining,
            "✅ [LSP DONE]"
        );

        let sym_list: Vec<_> = syms.into_iter().map(|s| {
            serde_json::json!({
                "name": s.name,
                "kind": lsp_symbol_kind(&s.kind),
                "location": {
                    "uri": file_uri(&s.path),
                    "range": {
                        "start": { "line": s.line.saturating_sub(1), "character": s.col.saturating_sub(1) },
                        "end": { "line": s.end_line.max(s.line).saturating_sub(1), "character": 0 }
                    }
                },
                "containerName": s.container
            })
        }).collect();
        let resp = serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": sym_list });
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

pub(crate) fn lsp_document_symbol(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let uri = params
        .get("textDocument")
        .and_then(|td| td.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let file_path = uri_or_path(&uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();

    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/documentSymbol",
        file = %file_path.display(),
        in_flight,
        "🚀 [LSP START]"
    );

    let engine_arc = Arc::clone(engine_lock);

    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let fp = fp_clone.clone();
        let syms = execute_bounded_query(
            &engine_arc,
            session_id,
            &fp_clone,
            is_single_owner,
            move |snapshot| {
                // An error is the answer, not an empty file: a README in a Rust workspace is
                // refused with the reason, and an agent must be able to tell that from a file
                // that declares nothing (#270).
                snapshot.document_symbols(&fp).map_err(|e| {
                    tracing::warn!(error = %e, session = session_id, "query failed");
                    e
                })
            },
        )
        .await
        .map_err(|e| e.to_string());

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let count = syms.as_ref().map_or(0, Vec::len);

        if ms > 200.0 {
            SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                req = req_num,
                session = session_id,
                method = "textDocument/documentSymbol",
                duration_ms = format!("{:.2}ms", ms),
                symbols = count,
                in_flight = remaining,
                "⚠️ [LSP SLOW >200ms]"
            );
        } else {
            tracing::info!(
                req = req_num,
                session = session_id,
                method = "textDocument/documentSymbol",
                duration_ms = format!("{:.2}ms", ms),
                symbols = count,
                in_flight = remaining,
                "✅ [LSP DONE]"
            );
        }

        let syms = match syms {
            Ok(syms) => syms,
            Err(message) => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "error": { "code": -32603, "message": message }
                });
                let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                return;
            }
        };
        let sym_list: Vec<_> = syms.into_iter().map(|s| {
             let kind_num = lsp_symbol_kind(&s.kind);
             serde_json::json!({
                 "name": s.name,
                 "kind": kind_num,
                 "location": {
                     "uri": uri,
                     "range": {
                         "start": { "line": s.line.saturating_sub(1), "character": s.col.saturating_sub(1) },
                         "end": { "line": s.end_line.max(s.line).saturating_sub(1), "character": 0 }
                     }
                 },
                 "containerName": if s.containers.is_empty() { s.detail.clone() } else { Some(s.containers.join(" > ")) }
             })
         }).collect();

        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": sym_list
        });
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}
