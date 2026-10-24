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
use serde_json::{Value, json};

/// A session over a workspace with no engine at all, and its output queue.
async fn bare_session() -> (
    tempfile::TempDir,
    SessionView,
    SharedOutputSender,
    rapidfire::mpsc::Receiver<super::super::shared_output::SharedOutputFrame>,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let workspace = Arc::new(workspace::SharedWorkspace::new(
        root.clone(),
        "generic-lsp".to_string(),
        None,
        None,
        None,
        None,
    ));
    let manager = workspace::WorkspaceManager::new();
    let lease = workspace::WorkspaceLease::acquire(workspace);
    let view = manager.register_session_view(1, root, lease).await;
    let (raw_tx, raw_rx) = rapidfire::mpsc::bounded(8);
    let out = SharedOutputSender::new(raw_tx, std::time::Duration::from_secs(1));
    (dir, view, out, raw_rx)
}

async fn next_payload(
    rx: &mut rapidfire::mpsc::Receiver<super::super::shared_output::SharedOutputFrame>,
) -> Value {
    match rx.recv().await.unwrap().message {
        WireMessage::LspPayload(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected a payload, got {other:?}"),
    }
}

#[test]
fn the_editor_is_told_the_server_capabilities_with_full_sync_and_only_the_servers_save_option() {
    let sync = json!({ "openClose": true, "change": 1 });
    let rust_defaults = editor_capabilities(None, true);
    assert_eq!(rust_defaults["hoverProvider"], true);
    assert_eq!(
        rust_defaults["completionProvider"]["triggerCharacters"],
        json!([":", ".", "'", "("])
    );
    assert_eq!(rust_defaults["textDocumentSync"], sync);
    for (name, server, rust, expected) in [
        (
            "none, not rust",
            None,
            false,
            json!({ "textDocumentSync": sync }),
        ),
        (
            "null, not rust",
            Some(Value::Null),
            false,
            json!({ "textDocumentSync": sync }),
        ),
        (
            "server caps kept",
            Some(json!({ "hoverProvider": true, "textDocumentSync": 2 })),
            true,
            json!({ "hoverProvider": true, "textDocumentSync": sync }),
        ),
        (
            "save option kept",
            Some(json!({ "textDocumentSync": { "change": 2, "save": { "includeText": true } } })),
            false,
            json!({ "textDocumentSync": { "openClose": true, "change": 1, "save": { "includeText": true } } }),
        ),
        (
            "empty server caps win over rust defaults",
            Some(json!({})),
            true,
            json!({ "textDocumentSync": sync }),
        ),
    ] {
        assert_eq!(editor_capabilities(server, rust), expected, "{name}");
    }
    let from_non_object = editor_capabilities(Some(json!("text")), true);
    assert_eq!(from_non_object, rust_defaults);
}

#[tokio::test]
async fn initialize_initialized_and_shutdown_are_answered_here_and_other_methods_pass_on() {
    let (_dir, view, out, mut rx) = bare_session().await;
    let translator = PathTranslator::new("/client/ws", "/server/ws");

    let flow = intercept_lifecycle_lsp(
        Some("initialize"),
        &Some(json!(5)),
        &view,
        &out,
        &translator,
    )
    .await;
    assert!(matches!(flow, Some(Flow::Next)));
    let reply = next_payload(&mut rx).await;
    assert_eq!(reply["id"], 5);
    assert_eq!(
        reply["result"]["capabilities"],
        json!({ "textDocumentSync": { "openClose": true, "change": 1 } })
    );
    assert_eq!(reply["result"]["serverInfo"]["name"], "prod-code");
    assert_eq!(
        reply["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );

    // Without an id the answer carries 1; a string id is kept as it came.
    intercept_lifecycle_lsp(Some("initialize"), &None, &view, &out, &translator).await;
    assert_eq!(next_payload(&mut rx).await["id"], 1);
    intercept_lifecycle_lsp(
        Some("shutdown"),
        &Some(json!("s-9")),
        &view,
        &out,
        &translator,
    )
    .await;
    assert_eq!(
        next_payload(&mut rx).await,
        json!({ "jsonrpc": "2.0", "id": "s-9", "result": null })
    );
    intercept_lifecycle_lsp(Some("shutdown"), &None, &view, &out, &translator).await;
    assert_eq!(next_payload(&mut rx).await["id"], 1);

    let flow = intercept_lifecycle_lsp(Some("initialized"), &None, &view, &out, &translator).await;
    assert!(matches!(flow, Some(Flow::Next)));
    for method in [Some("textDocument/hover"), Some("exit"), None] {
        let flow = intercept_lifecycle_lsp(method, &Some(json!(2)), &view, &out, &translator).await;
        assert!(flow.is_none(), "{method:?}");
    }
    drop(out);
    assert!(rx.recv().await.is_err(), "nothing else was sent");
}

#[tokio::test]
async fn without_any_engine_requests_get_a_null_answer_and_notifications_get_none() {
    let (_dir, view, out, mut rx) = bare_session().await;
    let translator = PathTranslator::new("/client/ws", "/server/ws");
    assert!(fallback_empty_lsp(&None, &view, &out).await.is_none());
    let flow = fallback_empty_lsp(&Some(json!(11)), &view, &out).await;
    assert!(matches!(flow, Some(Flow::Next)));
    assert_eq!(
        next_payload(&mut rx).await,
        json!({ "jsonrpc": "2.0", "id": 11, "result": null })
    );

    // Assists are for the Go and generic engines only; this workspace has neither.
    let request = json!({ "method": "prodCode/assists", "id": 3, "params": {} });
    for (method, id) in [
        (Some("prodCode/assists"), Some(json!(3))),
        (Some("prodCode/applyAssist"), Some(json!(3))),
        (Some("prodCode/assists"), None),
        (Some("textDocument/hover"), Some(json!(3))),
    ] {
        assert!(
            intercept_assists_for_managed(method, &id, &request, &view, &out, &translator)
                .is_none(),
            "{method:?} {id:?}"
        );
    }
    // Without a backend a document open or close is not intercepted.
    for method in ["textDocument/didOpen", "textDocument/didClose"] {
        let message =
            json!({ "method": method, "params": { "textDocument": { "uri": "file:///a.rs" } } });
        assert!(
            intercept_backend_open_or_close(Some(method), &message, &view)
                .await
                .is_none()
        );
    }
    drop(out);
    assert!(rx.recv().await.is_err());
}
