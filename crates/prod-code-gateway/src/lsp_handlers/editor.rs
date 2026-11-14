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

/// JSON-RPC `RequestCancelled`: the answer to a request its client cancelled.
const REQUEST_CANCELLED: i64 = -32800;

/// One of the requests an editor needs beyond navigation (completion, signature help, inlay
/// hints, highlights, code actions, formatting), answered by the in-memory Rust engine in the
/// session's view (#310).
///
/// It is answered on a snapshot, as a navigation request is: the engine's lock is held to take
/// the snapshot and no longer. Held for the whole answer, it queued every request of the
/// workspace one behind another, and behind each diagnostics pass of a large file: in one
/// editor session 504 of 1,753 requests waited 5 s or more, up to 50 s (#2018). A request the
/// editor cancels meanwhile (`$/cancelRequest`) is not computed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lsp_editor_request(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    cancels: &Arc<RequestCancels>,
    id: &Option<serde_json::Value>,
    method: &str,
    params: serde_json::Value,
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    if matches!(
        method,
        "rust-analyzer/viewSyntaxTree"
            | "rust-analyzer/expandMacro"
            | "textDocument/typeDefinition"
    ) {
        super::native_extensions::lsp_native_extension(
            out_tx,
            translator,
            view,
            id,
            method,
            params,
            engine_lock,
        );
        return;
    }
    let engine_arc = Arc::clone(engine_lock);
    let req_id = id.clone().unwrap_or(serde_json::json!(1));
    let out_tx_task = out_tx.clone();
    let translator_task = translator.clone();
    let session_id = view.session_id;
    let is_single_owner = view.is_single_owner();
    let method = method.to_string();
    let caps = view
        .editor_caps
        .lock()
        .map(|caps| caps.clone())
        .unwrap_or_default();
    let started = Instant::now();
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    // The session's root selects its view for every document, as for a native extension.
    let root = view.worktree_root.clone();
    let cancels = Arc::clone(cancels);
    let cancelled = cancels.begin(&req_id);
    tokio::task::spawn(async move {
        let outcome = {
            let m = method.clone();
            execute_cancellable_query(
                &engine_arc,
                session_id,
                &root,
                is_single_owner,
                cancelled,
                move |snapshot| {
                    snapshot
                        .editor_request_with(&m, &params, &caps)
                        .unwrap_or_else(|| Err(anyhow::anyhow!("{m} is not an editor request")))
                },
            )
            .await
        };
        cancels.finish(&req_id);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let resp = match outcome {
            Ok(result) => {
                tracing::info!(session = session_id, method = %method, duration_ms = format!("{ms:.2}ms"), "✅ [LSP DONE]");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "result": result })
            }
            Err(e) if e.is::<RequestCancelled>() => {
                tracing::debug!(session = session_id, method = %method, "editor request cancelled before it was computed");
                serde_json::json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": REQUEST_CANCELLED, "message": format!("{e}") } })
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

/// How many times a diagnostics pass that a write cancelled is run for the same edit.
const EDITOR_DIAGNOSTICS_ATTEMPTS: u32 = 5;

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
        for attempt in 1..=EDITOR_DIAGNOSTICS_ATTEMPTS {
            if latest(&edits) != Some(edit) {
                return;
            }
            let file = path.clone();
            let engine = Arc::clone(&engine_arc).lock_owned().await;
            let outcome = tokio::task::spawn_blocking(move || {
                let mut engine = engine;
                if !is_single_owner && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id) {
                        tracing::warn!(error = %e, session = session_id, "session view activation failed");
                    }
                let pass = engine.editor_diagnostics_pass(&file);
                // The pass has its snapshots. The engine is let go before the file is checked:
                // held through a pass over a large file, its lock kept every request of the
                // workspace, and the next edit, waiting for 7 to 16 s (#2018).
                drop(engine);
                pass.run()
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("diagnostics task failed: {e}")));
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
                    return;
                }
                // A write cancelled the pass and this document did not change: the write was to
                // another document, or another session's. The pass is run again on the new
                // state, a bounded number of times.
                Err(e)
                    if prod_code_engine_rust::is_salsa_cancelled(&e)
                        && attempt < EDITOR_DIAGNOSTICS_ATTEMPTS =>
                {
                    tokio::time::sleep(EDITOR_DIAGNOSTICS_DELAY).await;
                }
                Err(e) => {
                    tracing::warn!(session = session_id, file = %path.display(), error = %format!("{e:#}"), "editor diagnostics failed");
                    return;
                }
            }
        }
    });
}
