/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

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

/// Code of the diagnostic that stands for a file the analyzer panicked on.
pub const ANALYZER_PANIC: &str = prod_code_protocol::ANALYZER_PANIC_CODE;

/// The text of a panic payload, when it is one of the two shapes `panic!` produces.
pub(crate) fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "no message".to_string())
}

/// The diagnostics report for a file the analyzer panicked on: one error at its top that says
/// nothing in it was checked, and what check remains.
pub(crate) fn analyzer_panic_report(message: &str) -> serde_json::Value {
    let message = message.trim().trim_end_matches('.');
    serde_json::json!({ "kind": "full", "items": [ {
        "range": lsp_range(1, 1, 1, 1),
        "severity": 1,
        "code": ANALYZER_PANIC,
        "source": "prod-code",
        "message": format!(
            "rust-analyzer panicked while checking this file, so nothing in it was checked: \
             {message}. The compiler is the check that remains (`verify: \"compile\"`, or \
             `code_check`)."
        ),
    } ] })
}

pub(crate) fn lsp_safe_delete(
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
    tracing::info!(req = req_num, session = view.session_id, method = "prodCode/safeDelete", file = %file_path.display(), pos = format!("{line}:{col}"), in_flight, "🚀 [LSP START]");
    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();
    tokio::task::spawn(async move {
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                engine.safe_delete(&fp_clone, line, col)
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("safe delete task failed: {e}")))
        };
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let resp = match outcome {
            Ok(Ok(outcome)) => {
                tracing::info!(
                    req = req_num,
                    session = session_id,
                    method = "prodCode/safeDelete",
                    duration_ms = format!("{:.2}ms", ms),
                    in_flight = remaining,
                    "✅ [LSP DONE]"
                );
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": workspace_edit_json(&outcome) })
            }
            Ok(Err(refused)) => {
                tracing::info!(
                    req = req_num,
                    session = session_id,
                    method = "prodCode/safeDelete",
                    "🚫 [LSP REFUSED]"
                );
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32602, "message": refused } })
            }
            Err(e) => {
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": e.to_string() } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

pub(crate) fn lsp_structural_replace(
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
    let (line, col) = params
        .get("position")
        .map(|position| one_based_position(Some(position)))
        .transpose()
        .unwrap_or(Some((1, 1)))
        .unwrap_or((1, 1));
    let rule = params
        .get("rule")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();
    let scope = params
        .get("scope")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(uri_or_path);
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "prodCode/structuralReplace",
        file = %file_path.display(),
        pos = format!("{line}:{col}"),
        rule = %rule,
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
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                engine.structural_replace(&rule, &fp_clone, line, col, scope.as_deref())
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("codemod task failed: {e}")))
        };
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let resp = match outcome {
            Ok(Ok(outcome)) => {
                tracing::info!(
                    req = req_num,
                    session = session_id,
                    method = "prodCode/structuralReplace",
                    duration_ms = format!("{:.2}ms", ms),
                    files = outcome.files.len(),
                    edits = outcome.total_edits(),
                    moves = outcome.moves.len(),
                    in_flight = remaining,
                    "✅ [LSP DONE]"
                );
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": workspace_edit_json(&outcome) })
            }
            Ok(Err(refused)) => {
                tracing::info!(req = req_num, session = session_id, method = "prodCode/structuralReplace", reason = %refused, "🚫 [LSP REFUSED]");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32602, "message": refused } })
            }
            Err(e) => {
                tracing::warn!(req = req_num, session = session_id, error = %e, "codemod failed");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": e.to_string() } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

pub(crate) fn lsp_rename(
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
    let new_name = params
        .get("newName")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    tracing::info!(
        req = req_num,
        session = view.session_id,
        method = "textDocument/rename",
        file = %file_path.display(),
        pos = format!("{line}:{col}"),
        new_name = %new_name,
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
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                engine.rename(&fp_clone, line, col, &new_name)
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("rename task failed: {e}")))
        };
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let resp = match outcome {
            Ok(Ok(outcome)) => {
                tracing::info!(
                    req = req_num,
                    session = session_id,
                    method = "textDocument/rename",
                    duration_ms = format!("{:.2}ms", ms),
                    files = outcome.files.len(),
                    edits = outcome.total_edits(),
                    moves = outcome.moves.len(),
                    in_flight = remaining,
                    "✅ [LSP DONE]"
                );
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": workspace_edit_json(&outcome) })
            }
            Ok(Err(refused)) => {
                tracing::info!(req = req_num, session = session_id, method = "textDocument/rename", reason = %refused, "🚫 [LSP REFUSED]");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32602, "message": refused } })
            }
            Err(e) => {
                tracing::warn!(req = req_num, session = session_id, error = %e, "rename failed");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": e.to_string() } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

pub(crate) fn lsp_assists(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    method: Option<&str>,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let apply = method == Some("prodCode/applyAssist");
    let uri = params
        .get("textDocument")
        .and_then(|td| td.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    let (line, col) = one_based_position(params.pointer("/range/start")).unwrap_or((1, 1));
    let end = params
        .pointer("/range/end")
        .and_then(|end| one_based_position(Some(end)).ok());
    let assist_id = params
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let subtype = params
        .get("subtype")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let file_path = uri_or_path(uri);

    let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    let query_start = Instant::now();
    let method_name = if apply {
        "prodCode/applyAssist"
    } else {
        "prodCode/assists"
    };
    tracing::info!(req = req_num, session = view.session_id, method = method_name, file = %file_path.display(), pos = format!("{line}:{col}"), assist = %assist_id, in_flight, "🚀 [LSP START]");

    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let fp_clone = file_path.clone();
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();

    tokio::task::spawn(async move {
        let result = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                if apply {
                    engine
                        .apply_assist(&fp_clone, line, col, end, &assist_id, subtype)
                        .map(|r| r.map(|outcome| workspace_edit_json(&outcome)))
                } else {
                    engine
                        .list_assists(&fp_clone, line, col, end)
                        .map(|list| Ok(serde_json::json!(list)))
                }
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("assist task failed: {e}")))
        };
        let ms = query_start.elapsed().as_secs_f64() * 1000.0;
        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
        let resp = match result {
            Ok(Ok(value)) => {
                tracing::info!(
                    req = req_num,
                    session = session_id,
                    method = method_name,
                    duration_ms = format!("{:.2}ms", ms),
                    in_flight = remaining,
                    "✅ [LSP DONE]"
                );
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": value })
            }
            Ok(Err(refused)) => {
                tracing::info!(req = req_num, session = session_id, method = method_name, reason = %refused, "🚫 [LSP REFUSED]");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32602, "message": refused } })
            }
            Err(e) => {
                tracing::warn!(req = req_num, session = session_id, error = %e, "assist failed");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": e.to_string() } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

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

/// One of the requests an editor needs beyond navigation (completion, signature help, inlay
/// hints, highlights, code actions, formatting), answered by the in-memory Rust engine in the
/// session's view (#310).
pub(crate) fn lsp_editor_request(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    method: &str,
    params: serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();
    let method = method.to_string();
    let started = Instant::now();
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    tokio::task::spawn(async move {
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            let m = method.clone();
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                engine
                    .editor_request(&m, &params)
                    .unwrap_or_else(|| Err(anyhow::anyhow!("{m} is not an editor request")))
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("query task failed: {e}")))
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let resp = match outcome {
            Ok(result) => {
                tracing::info!(session = session_id, method = %method, duration_ms = format!("{ms:.2}ms"), "✅ [LSP DONE]");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": result })
            }
            Err(e) => {
                tracing::warn!(session = session_id, method = %method, error = %format!("{e:#}"), "editor request failed");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32603, "message": format!("{e:#}") } })
            }
        };
        let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
    });
}

/// How long an editor session's document must stay unchanged before its diagnostics are
/// computed: a burst of typing costs one pass, for its last edit.
const EDITOR_DIAGNOSTICS_DELAY: Duration = Duration::from_millis(300);

/// Pushes the diagnostics of `path` to an editor session once the document has stopped
/// changing for [`EDITOR_DIAGNOSTICS_DELAY`] (#310). Sessions of agents and tools ask for
/// diagnostics when they want them and get none pushed.
pub(crate) fn publish_rust_diagnostics(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    meta: &Arc<SessionMeta>,
    path: PathBuf,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    if !meta.editor {
        return;
    }
    let edit = {
        let mut edits = meta.edits.lock().unwrap_or_else(|e| e.into_inner());
        let count = edits.entry(path.clone()).or_default();
        *count += 1;
        *count
    };
    let edits = Arc::clone(&meta.edits);
    let engine_arc = Arc::clone(engine_lock);
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();
    tokio::task::spawn(async move {
        tokio::time::sleep(EDITOR_DIAGNOSTICS_DELAY).await;
        let latest = |edits: &std::sync::Mutex<std::collections::HashMap<PathBuf, u64>>| {
            edits
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&path)
                .copied()
        };
        if latest(&edits) != Some(edit) {
            return;
        }
        let file = path.clone();
        let outcome = {
            let mut engine = engine_arc.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                engine.editor_diagnostics(&file)
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("diagnostics task failed: {e}")))
        };
        // An edit that arrived during the pass gets a pass of its own.
        if latest(&edits) != Some(edit) {
            return;
        }
        match outcome {
            Ok(diagnostics) => {
                let note = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": { "uri": file_uri(&path), "diagnostics": diagnostics }
                });
                let client_note = translator_task.translate_lsp_to_client(&note.to_string());
                let _ = out_tx_task.send(WireMessage::LspPayload(client_note)).await;
            }
            Err(e) => {
                tracing::warn!(session = session_id, file = %path.display(), error = %format!("{e:#}"), "editor diagnostics failed");
            }
        }
    });
}
