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

pub(crate) fn lsp_call_hierarchy(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    hm: &str,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    // Call-hierarchy follow-ups carry the item; the others a text document position.
    let (uri, position) = match params.get("item") {
        Some(item) => (
            item.get("uri").and_then(|u| u.as_str()).unwrap_or(""),
            item.get("selectionRange").and_then(|r| r.get("start")),
        ),
        None => (
            params
                .get("textDocument")
                .and_then(|td| td.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or(""),
            params.get("position"),
        ),
    };
    let (line, col) = one_based_position(position).unwrap_or((1, 1));
    let file_path = uri_or_path(uri);
    let method_name = hm.to_string();

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(req = req_num, session = view.session_id, method = %method_name, file = %file_path.display(), pos = format!("{line}:{col}"), in_flight, "🚀 [LSP START]");

    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let m = method_name.clone();
        let fp = fp_clone.clone();
        let outcome = execute_bounded_query(
            &engine_arc,
            session_id,
            &fp_clone,
            is_single_owner,
            move |snapshot| hierarchy_query(snapshot, &m, &fp, line, col),
        )
        .await;
        let outcome = match outcome {
            Err(ref e)
                if method_name == "textDocument/diagnostic"
                    && e.to_string().starts_with("analyzer panic: ") =>
            {
                let msg = e.to_string();
                let msg = msg.strip_prefix("analyzer panic: ").unwrap_or(&msg);
                Ok(analyzer_panic_report(msg))
            }
            other => other,
        };
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let resp = match outcome {
            Ok(result) => {
                tracing::info!(req = req_num, session = session_id, method = %method_name, duration_ms = format!("{:.2}ms", ms), items = result.as_array().map(|a| a.len()).unwrap_or(0), in_flight = remaining, "✅ [LSP DONE]");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": result })
            }
            Err(e) => {
                tracing::warn!(req = req_num, session = session_id, method = %method_name, error = %e, "query failed");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": e.to_string() } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

pub(crate) fn lsp_references(
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
        .unwrap_or("");
    let (line, col) = one_based_position(params.get("position")).unwrap_or((1, 1));
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();

    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/references",
        file = %file_path.display(),
        pos = format!("{line}:{col}"),
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
        let refs = execute_bounded_query(&engine_arc, session_id, &fp_clone, is_single_owner, {
            let fp = fp_clone.clone();
            move |snapshot| snapshot.find_all_refs(&fp, line, col)
        })
        .await;

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let count = refs.as_ref().map_or(0, Vec::len);

        if ms > 200.0 {
            SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                req = req_num,
                session = session_id,
                method = "textDocument/references",
                duration_ms = format!("{:.2}ms", ms),
                references = count,
                in_flight = remaining,
                "⚠️ [LSP SLOW >200ms]"
            );
        } else {
            tracing::info!(
                req = req_num,
                session = session_id,
                method = "textDocument/references",
                duration_ms = format!("{:.2}ms", ms),
                references = count,
                in_flight = remaining,
                "✅ [LSP DONE]"
            );
        }

        let locations = refs.map(|targets| targets.into_iter().map(|t| {
            serde_json::json!({
                "uri": file_uri(&t.path),
                "range": {
                    "start": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) },
                    "end": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) }
                }
            })
        }).collect::<Vec<_>>());

        let resp = match locations {
            Ok(locations) => {
                serde_json::json!({"jsonrpc": "2.0", "id": req_id, "result": locations})
            }
            Err(error) => serde_json::json!({
                "jsonrpc": "2.0", "id": req_id,
                "error": { "code": -32603, "message": error.to_string() }
            }),
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

pub(crate) fn lsp_definition(
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
        .unwrap_or("");
    let (line, col) = one_based_position(params.get("position")).unwrap_or((1, 1));
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();

    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/definition",
        file = %file_path.display(),
        pos = format!("{line}:{col}"),
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
        let defs = execute_bounded_query(&engine_arc, session_id, &fp_clone, is_single_owner, {
            let fp = fp_clone.clone();
            move |snapshot| snapshot.goto_definition(&fp, line, col)
        })
        .await;

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let count = defs.as_ref().map_or(0, Vec::len);

        if ms > 200.0 {
            SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                req = req_num,
                session = session_id,
                method = "textDocument/definition",
                duration_ms = format!("{:.2}ms", ms),
                targets = count,
                in_flight = remaining,
                "⚠️ [LSP SLOW >200ms]"
            );
        } else {
            tracing::info!(
                req = req_num,
                session = session_id,
                method = "textDocument/definition",
                duration_ms = format!("{:.2}ms", ms),
                targets = count,
                in_flight = remaining,
                "✅ [LSP DONE]"
            );
        }

        let locations = defs.map(|targets| targets.into_iter().map(|t| {
            serde_json::json!({
                "uri": file_uri(&t.path),
                "range": {
                    "start": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) },
                    "end": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) }
                }
            })
        }).collect::<Vec<_>>());

        let resp = match locations {
            Ok(locations) => {
                serde_json::json!({"jsonrpc": "2.0", "id": req_id, "result": locations})
            }
            Err(error) => serde_json::json!({
                "jsonrpc": "2.0", "id": req_id,
                "error": { "code": -32603, "message": error.to_string() }
            }),
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

pub(crate) fn lsp_hover(
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
        .unwrap_or("");
    let (line, col) = one_based_position(params.get("position")).unwrap_or((1, 1));
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();

    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/hover",
        file = %file_path.display(),
        pos = format!("{line}:{col}"),
        in_flight,
        "🚀 [LSP START]"
    );

    // Acquire cheap snapshot (<1 µs) without holding mutex during query
    let engine_arc = Arc::clone(engine_lock);

    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let hover_res =
            execute_bounded_query(&engine_arc, session_id, &fp_clone, is_single_owner, {
                let fp = fp_clone.clone();
                move |snapshot| snapshot.hover(&fp, line, col)
            })
            .await;

        let duration = query_start.elapsed();
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let ms = duration.as_secs_f64() * 1000.0;
        let found = matches!(&hover_res, Ok(Some(_)));

        if ms > 200.0 {
            SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                req = req_num,
                session = session_id,
                method = "textDocument/hover",
                duration_ms = format!("{:.2}ms", ms),
                found,
                in_flight = remaining,
                "⚠️ [LSP SLOW >200ms]"
            );
        } else {
            tracing::info!(
                req = req_num,
                session = session_id,
                method = "textDocument/hover",
                duration_ms = format!("{:.2}ms", ms),
                found,
                in_flight = remaining,
                "✅ [LSP DONE]"
            );
        }

        let resp = match hover_res {
            Ok(Some(markup)) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "contents": {
                        "kind": "markdown",
                        "value": markup
                    }
                }
            }),
            Ok(None) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": null
            }),
            Err(error) => serde_json::json!({
                "jsonrpc": "2.0", "id": req_id,
                "error": { "code": -32603, "message": error.to_string() }
            }),
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}
