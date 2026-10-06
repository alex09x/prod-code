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
use std::sync::atomic::Ordering;
use std::time::Instant;

pub fn editor_capabilities(server: Option<serde_json::Value>, rust: bool) -> serde_json::Value {
    let mut caps = match server {
        Some(caps) if caps.is_object() => caps,
        _ if rust => serde_json::json!({
            "hoverProvider": true,
            "definitionProvider": true,
            "referencesProvider": true,
            "implementationProvider": true,
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
            "renameProvider": true,
            "callHierarchyProvider": true,
            "completionProvider": {
                "triggerCharacters": [":", ".", "'", "("],
                "resolveProvider": true
            },
            "signatureHelpProvider": { "triggerCharacters": ["(", ",", "<"] },
            "inlayHintProvider": true,
            "documentHighlightProvider": true,
            "codeActionProvider": { "resolveProvider": true },
            "documentFormattingProvider": true
        }),
        _ => serde_json::json!({}),
    };
    let save = caps.pointer("/textDocumentSync/save").cloned();
    caps["textDocumentSync"] = serde_json::json!({ "openClose": true, "change": 1 });
    if let Some(save) = save {
        caps["textDocumentSync"]["save"] = save;
    }
    caps
}

pub async fn intercept_lifecycle_lsp(
    method: Option<&str>,
    id: &Option<serde_json::Value>,
    view: &SessionView,
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
) -> Option<Flow> {
    if method == Some("initialize") {
        let req_id = id.clone().unwrap_or(serde_json::json!(1));
        let caps = if let Some(ref go) = view.workspace.go_engine {
            go.capabilities.read().await.clone()
        } else if let Some(ref generic_eng) = view.workspace.generic_engine {
            generic_eng.capabilities.read().await.clone()
        } else if let Some(ref backend) = view.workspace.backend {
            backend.capabilities.read().await.clone()
        } else {
            None
        };
        let caps = editor_capabilities(caps, view.workspace.rust_engine.is_some());
        let init_resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "capabilities": caps,
                "serverInfo": {
                    "name": "prod-code",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        });
        let client_resp = translator.translate_lsp_to_client(&init_resp.to_string());
        let _ = out_tx.send(WireMessage::LspPayload(client_resp)).await;
        return Some(Flow::Next);
    }

    if method == Some("initialized") {
        return Some(Flow::Next);
    }

    if method == Some("shutdown") {
        let req_id = id.clone().unwrap_or(serde_json::json!(1));
        let shutdown_resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": null
        });
        let _ = out_tx
            .send(WireMessage::LspPayload(shutdown_resp.to_string()))
            .await;
        return Some(Flow::Next);
    }

    None
}

pub async fn intercept_backend_open_or_close(
    method: Option<&str>,
    val: &serde_json::Value,
    view: &SessionView,
) -> Option<Flow> {
    if let (Some("textDocument/didOpen"), Some(backend)) = (method, &view.workspace.backend) {
        let uri = val
            .get("params")
            .and_then(|p| p.get("textDocument"))
            .and_then(|td| td.get("uri"))
            .and_then(|u| u.as_str())
            .unwrap_or("");
        let is_open = backend.open_files.read().await.contains(uri);
        if is_open {
            let text = val
                .get("params")
                .and_then(|p| p.get("textDocument"))
                .and_then(|td| td.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let version = val
                .get("params")
                .and_then(|p| p.get("textDocument"))
                .and_then(|td| td.get("version"))
                .and_then(|v| v.as_i64())
                .unwrap_or(2);
            let did_change = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didChange",
                "params": {
                    "textDocument": {
                        "uri": uri,
                        "version": version
                    },
                    "contentChanges": [
                        { "text": text }
                    ]
                }
            });
            let _ = backend.send_lsp(&did_change.to_string()).await;
            return Some(Flow::Next);
        } else {
            backend.open_files.write().await.insert(uri.to_string());
        }
    }

    if let (Some("textDocument/didClose"), Some(backend)) = (method, &view.workspace.backend) {
        let uri = val
            .get("params")
            .and_then(|p| p.get("textDocument"))
            .and_then(|td| td.get("uri"))
            .and_then(|u| u.as_str())
            .unwrap_or("");
        backend.open_files.write().await.remove(uri);
    }

    None
}

pub fn intercept_assists_for_managed(
    method: Option<&str>,
    id: &Option<serde_json::Value>,
    val: &serde_json::Value,
    view: &SessionView,
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
) -> Option<Flow> {
    if let (Some(pm @ ("prodCode/assists" | "prodCode/applyAssist")), Some(req_id)) = (method, id)
        && (view.workspace.go_engine.is_some() || view.workspace.generic_engine.is_some())
    {
        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
        let go = view.workspace.go_engine.clone();
        let generic = view.workspace.generic_engine.clone();
        let out_tx_task = out_tx.clone();
        let translator_task = translator.clone();
        let r_id = req_id.clone();
        let session_id = view.session_id;
        let method_name = pm.to_string();
        let start = Instant::now();
        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
        tokio::task::spawn(async move {
            let engine = match (&go, &generic) {
                (_, Some(g)) => ManagedLsp::Generic(g),
                (Some(g), None) => ManagedLsp::Go(g),
                (None, None) => unreachable!("guarded above"),
            };
            let outcome = lsp_code_actions(&engine, &method_name, params).await;
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            let resp = match outcome {
                Ok(result) => {
                    tracing::info!(
                        session = session_id,
                        method = %method_name,
                        duration_ms = format!("{ms:.2}ms"),
                        "✅ [LSP DONE] code actions"
                    );
                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "result": result })
                }
                Err(err) => {
                    tracing::info!(
                        session = session_id,
                        method = %method_name,
                        error = %err,
                        "🚫 [LSP REFUSED] code actions"
                    );
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": r_id,
                        "error": { "code": -32602, "message": err.to_string() }
                    })
                }
            };
            let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
            let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
        });
        return Some(Flow::Next);
    }
    None
}

pub async fn fallback_empty_lsp(
    id: &Option<serde_json::Value>,
    view: &SessionView,
    out_tx: &SharedOutputSender,
) -> Option<Flow> {
    if let (Some(req_id), None, None, None, None) = (
        id,
        &view.workspace.backend,
        &view.workspace.rust_engine,
        &view.workspace.go_engine,
        &view.workspace.generic_engine,
    ) {
        let empty_resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": null
        });
        let _ = out_tx
            .send(WireMessage::LspPayload(empty_resp.to_string()))
            .await;
        return Some(Flow::Next);
    }
    None
}
