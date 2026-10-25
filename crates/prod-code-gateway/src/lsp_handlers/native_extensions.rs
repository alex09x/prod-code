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

/// Native extensions share the navigation query's bounded session/worktree snapshot path.
/// In particular, validation workspaces have no external backend to forward requests to.
pub(super) fn lsp_native_extension(
    output: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    id: &Option<serde_json::Value>,
    method: &str,
    params: serde_json::Value,
    engine: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
) {
    let Some(id) = id.as_ref().filter(|id| !id.is_null()).cloned() else {
        return;
    };
    let output = output.clone();
    let translator = translator.clone();
    let session_id = view.session_id;
    let single_owner = view.is_single_owner();
    let engine = Arc::clone(engine);
    let method = method.to_owned();
    let root = view.worktree_root.clone();
    let timeout = semantic_query_budget(&params);
    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
    tokio::spawn(async move {
        // Use the session root for preparation even for malformed document parameters; this
        // avoids treating an empty URI as a relative path and always selects the right view.
        let result = execute_bounded_query_with_timeout(
            &engine,
            session_id,
            &root,
            single_owner,
            timeout,
            move |snapshot| {
                snapshot
                    .editor_request(&method, &params)
                    .expect("native extension is registered in EDITOR_METHODS")
            },
        )
        .await;
        let response = match result {
            Ok(result) => serde_json::json!({"jsonrpc":"2.0", "id":id, "result":result}),
            Err(error) => {
                let code = if error.is::<prod_code_engine_rust::editor::InvalidExtensionParams>() {
                    -32602
                } else {
                    -32603
                };
                serde_json::json!({"jsonrpc":"2.0", "id":id,
                    "error":{"code":code, "message":format!("{error:#}")}})
            }
        };
        let response = translator.translate_lsp_to_client(&response.to_string());
        let _ = output.send(WireMessage::LspPayload(response)).await;
    });
}
