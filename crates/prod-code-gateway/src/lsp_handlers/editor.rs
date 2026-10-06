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
