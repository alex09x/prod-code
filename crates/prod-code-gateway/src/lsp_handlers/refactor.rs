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
        let outcome = execute_bounded_query(&engine_arc, session_id, &fp_clone, is_single_owner, {
            let fp = fp_clone.clone();
            move |snapshot| snapshot.safe_delete(&fp, line, col)
        })
        .await;
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
        let outcome = execute_bounded_query(&engine_arc, session_id, &fp_clone, is_single_owner, {
            let fp = fp_clone.clone();
            let nn = new_name.clone();
            move |snapshot| snapshot.rename(&fp, line, col, &nn)
        })
        .await;
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
        let result = execute_bounded_query(&engine_arc, session_id, &fp_clone, is_single_owner, {
            let fp = fp_clone.clone();
            let aid = assist_id.clone();
            move |snapshot| {
                if apply {
                    snapshot
                        .apply_assist(&fp, line, col, end, &aid, subtype)
                        .map(|r| r.map(|outcome| workspace_edit_json(&outcome)))
                } else {
                    snapshot
                        .list_assists(&fp, line, col, end)
                        .map(|list| Ok(serde_json::json!(list)))
                }
            }
        })
        .await;
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
