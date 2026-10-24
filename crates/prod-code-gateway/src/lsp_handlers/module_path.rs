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

/// Resolve relative Rust paths using the analyzer's module identity in this session view.
pub(crate) fn lsp_module_path(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    params: &serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let path = uri_or_path(
        params
            .pointer("/textDocument/uri")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    let engine = Arc::clone(engine_lock);
    let session = view.session_id;
    let root = view.workspace.root.clone();
    let single_owner = view.is_single_owner();
    let id = id.clone();
    let translator = translator.clone();
    let out_tx = out_tx.clone();
    tokio::spawn(async move {
        let result =
            execute_bounded_query(&engine, session, &root, single_owner, move |snapshot| {
                snapshot.module_path_for_file(&path)
            })
            .await;
        let response = match result {
            Ok(module) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": module }),
            Err(error) => {
                serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": format!("{error:#}") } })
            }
        };
        let response = translator.translate_lsp_to_client(&response.to_string());
        let _ = out_tx.send(WireMessage::LspPayload(response)).await;
    });
}
