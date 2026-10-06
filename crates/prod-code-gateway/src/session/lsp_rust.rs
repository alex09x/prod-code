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
use super::shared_output::SharedOutputSender;
use crate::*;
use std::sync::Arc;
use std::time::Instant;

pub async fn handle_rust_lsp(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    meta: &Arc<SessionMeta>,
    method: Option<&str>,
    id: &Option<serde_json::Value>,
    val: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) -> Option<Flow> {
    match method {
        Some("textDocument/hover") => {
            if let Some(params) = val.get("params") {
                lsp_hover(out_tx, translator, view, id, params, engine_lock);
                return Some(Flow::Next);
            }
        }
        Some("textDocument/definition") => {
            if let Some(params) = val.get("params") {
                lsp_definition(out_tx, translator, view, id, params, engine_lock);
                return Some(Flow::Next);
            }
        }
        Some("textDocument/references") => {
            if let Some(params) = val.get("params") {
                lsp_references(out_tx, translator, view, id, params, engine_lock);
                return Some(Flow::Next);
            }
        }
        Some("textDocument/documentSymbol") => {
            if let Some(params) = val.get("params") {
                lsp_document_symbol(out_tx, translator, view, id, params, engine_lock);
                return Some(Flow::Next);
            }
        }
        Some("workspace/symbol") => {
            if let Some(params) = val.get("params") {
                lsp_workspace_symbol(out_tx, translator, view, id, params, engine_lock);
                return Some(Flow::Next);
            }
        }
        Some("prodCode/assists") | Some("prodCode/applyAssist") => {
            if let Some(params) = val.get("params") {
                lsp_assists(out_tx, translator, view, method, id, params, engine_lock);
                return Some(Flow::Next);
            }
        }
        Some("prodCode/safeDelete") => {
            if let Some(params) = val.get("params") {
                lsp_safe_delete(out_tx, translator, view, id, params, engine_lock);
                return Some(Flow::Next);
            }
        }
        Some(
            hm @ ("textDocument/prepareCallHierarchy"
            | "callHierarchy/incomingCalls"
            | "callHierarchy/outgoingCalls"
            | "textDocument/implementation"
            | "textDocument/diagnostic"),
        ) => {
            if let Some(params) = val.get("params") {
                lsp_call_hierarchy(out_tx, translator, view, id, hm, params, engine_lock);
                return Some(Flow::Next);
            }
        }
        Some("textDocument/rename") => {
            if let Some(params) = val.get("params") {
                lsp_rename(out_tx, translator, view, id, params, engine_lock);
                return Some(Flow::Next);
            }
        }
        Some("prodCode/structuralReplace") => {
            if let Some(params) = val.get("params") {
                lsp_structural_replace(out_tx, translator, view, id, params, engine_lock);
                return Some(Flow::Next);
            }
        }
        Some(m) if prod_code_engine_rust::editor::EDITOR_METHODS.contains(&m) => {
            let params = val.get("params").cloned().unwrap_or_default();
            lsp_editor_request(out_tx, translator, view, id, m, params, engine_lock);
            return Some(Flow::Next);
        }
        Some("textDocument/didOpen") => {
            if let Some(params) = val.get("params") {
                let uri = params
                    .get("textDocument")
                    .and_then(|td| td.get("uri"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("");
                let file_path = uri_or_path(uri);
                if let Some(text) = params
                    .get("textDocument")
                    .and_then(|td| td.get("text"))
                    .and_then(|t| t.as_str())
                {
                    let edit_start = Instant::now();
                    let text_len = text.len();
                    {
                        let mut engine = engine_lock.lock().await;
                        if view.is_single_owner() {
                            if let Err(e) =
                                engine.apply_file_change(&file_path, text.to_string())
                            {
                                tracing::warn!(
                                    error = %e,
                                    file = %file_path.display(),
                                    "direct-edit didOpen file change failed"
                                );
                            }
                            if let Ok(mut files) = view.direct_edit_open_files.lock() {
                                files.insert(file_path.clone(), text.to_string());
                            }
                        } else if let Err(e) = engine.set_session_overlay(
                            view.session_id,
                            &file_path,
                            Some(text.to_string()),
                        ) {
                            tracing::warn!(
                                error = %e,
                                file = %file_path.display(),
                                "session overlay update failed"
                            );
                        }
                    }
                    let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                    tracing::info!(
                        session = view.session_id,
                        file = %file_path.display(),
                        bytes = text_len,
                        duration_ms = format!("{:.2}ms", ms),
                        single_owner = view.is_single_owner(),
                        "📝 [EDIT] didOpen recorded in Salsa DB"
                    );
                    publish_rust_diagnostics(
                        out_tx,
                        translator,
                        view,
                        meta,
                        file_path.clone(),
                        engine_lock,
                    );
                }
            }
        }
        Some("textDocument/didChange") => {
            if let Some(params) = val.get("params") {
                let uri = params
                    .get("textDocument")
                    .and_then(|td| td.get("uri"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("");
                let file_path = uri_or_path(uri);
                let first = params
                    .get("contentChanges")
                    .and_then(|c| c.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|c| c.get("text"))
                    .and_then(|t| t.as_str());
                if let Some(text) = first {
                    let edit_start = Instant::now();
                    let text_len = text.len();
                    {
                        let mut engine = engine_lock.lock().await;
                        if view.is_single_owner() {
                            if let Err(e) =
                                engine.apply_file_change(&file_path, text.to_string())
                            {
                                tracing::warn!(
                                    error = %e,
                                    file = %file_path.display(),
                                    "direct-edit didChange file change failed"
                                );
                            }
                            if let Ok(mut files) = view.direct_edit_open_files.lock() {
                                files.insert(file_path.clone(), text.to_string());
                            }
                        } else if let Err(e) = engine.set_session_overlay(
                            view.session_id,
                            &file_path,
                            Some(text.to_string()),
                        ) {
                            tracing::warn!(
                                error = %e,
                                file = %file_path.display(),
                                "session overlay update failed"
                            );
                        }
                    }
                    let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                    tracing::info!(
                        session = view.session_id,
                        file = %file_path.display(),
                        bytes = text_len,
                        duration_ms = format!("{:.2}ms", ms),
                        single_owner = view.is_single_owner(),
                        "📝 [EDIT] didChange recorded in Salsa DB"
                    );
                    publish_rust_diagnostics(
                        out_tx,
                        translator,
                        view,
                        meta,
                        file_path.clone(),
                        engine_lock,
                    );
                }
            }
        }
        Some("textDocument/didClose") => {
            if let Some(params) = val.get("params") {
                let uri = params
                    .get("textDocument")
                    .and_then(|td| td.get("uri"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("");
                let file_path = uri_or_path(uri);
                let mut engine = engine_lock.lock().await;
                if view.is_single_owner() {
                    if let Err(e) = engine.reload_file(&file_path) {
                        tracing::warn!(
                            error = %e,
                            file = %file_path.display(),
                            "direct-edit didClose reload failed"
                        );
                    }
                    if let Ok(mut files) = view.direct_edit_open_files.lock() {
                        files.remove(&file_path);
                    }
                } else if let Err(e) =
                    engine.clear_session_overlay(view.session_id, &file_path)
                {
                    tracing::warn!(
                        error = %e,
                        file = %file_path.display(),
                        "session overlay close failed"
                    );
                }
            }
            return Some(Flow::Next);
        }
        // A request the in-memory engine has no answer for is refused rather
        // than left without a reply, which an editor waits on for good.
        Some(m) if id.as_ref().is_some_and(|i| !i.is_null()) => {
            let refused = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("{m} is not supported by prod-code for Rust") }
            });
            let _ = out_tx.send(WireMessage::LspPayload(refused.to_string())).await;
            return Some(Flow::Next);
        }
        _ => {}
    }
    None
}
