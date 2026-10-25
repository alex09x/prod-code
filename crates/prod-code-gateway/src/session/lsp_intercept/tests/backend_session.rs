/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Sessions over a managed backend: the capabilities it announced, and a document the editor
//! opens twice reaching it as a full-text change.

use super::*;
use crate::backend::BackendWorker;
use std::time::Duration;

async fn scripted_backend(dir: &std::path::Path) -> Arc<BackendWorker> {
    let args = crate::lsp::test_support::scripted_server_args(dir, json!({}));
    Arc::new(
        BackendWorker::spawn_command(
            dir,
            "python",
            "python3",
            &args,
            Duration::from_secs(30),
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .await
        .unwrap(),
    )
}

/// The notifications the scripted backend has received so far, in order.
async fn received(backend: &BackendWorker, id: u64) -> Value {
    let mut replies = backend.subscribe();
    backend
        .send_lsp(&json!({ "jsonrpc": "2.0", "id": id, "method": "test/events" }).to_string())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let text = replies.recv().await.expect("the backend stays open");
            let value: Value = serde_json::from_str(&text).unwrap();
            if value["id"] == id {
                return value["result"].clone();
            }
        }
    })
    .await
    .expect("the backend answers")
}

fn open(uri: &str, text: Option<&str>, version: Option<i64>) -> Value {
    let mut document = json!({ "uri": uri });
    if let Some(text) = text {
        document["text"] = json!(text);
    }
    if let Some(version) = version {
        document["version"] = json!(version);
    }
    json!({ "method": "textDocument/didOpen", "params": { "textDocument": document } })
}

#[tokio::test]
async fn a_backend_session_is_told_the_backends_capabilities_and_repeated_opens_become_changes() {
    let (dir, mut view, out, mut rx) = bare_session().await;
    let backend = scripted_backend(dir.path()).await;
    view.workspace = Arc::new(workspace::SharedWorkspace::new(
        view.workspace.root.clone(),
        "python".into(),
        None,
        None,
        None,
        Some(Arc::clone(&backend)),
    ));
    let translator = PathTranslator::new("/client/ws", "/server/ws");

    let flow = intercept_lifecycle_lsp(
        Some("initialize"),
        &Some(json!(3)),
        &view,
        &out,
        &translator,
    )
    .await;
    assert!(matches!(flow, Some(Flow::Next)));
    assert_eq!(
        next_payload(&mut rx).await["result"]["capabilities"],
        json!({
            "diagnosticProvider": { "interFileDependencies": false, "workspaceDiagnostics": false },
            "textDocumentSync": { "openClose": true, "change": 1 }
        })
    );
    // Neither the empty-answer fallback nor the managed assists apply with a backend.
    assert!(
        fallback_empty_lsp(&Some(json!(4)), &view, &out)
            .await
            .is_none()
    );
    let assists = json!({ "method": "prodCode/assists", "params": {} });
    assert!(
        intercept_assists_for_managed(
            Some("prodCode/assists"),
            &Some(json!(5)),
            &assists,
            &view,
            &out,
            &translator
        )
        .is_none()
    );

    let uri = "file:///server/ws/a.py";
    let first = open(uri, Some("one"), Some(1));
    assert!(
        intercept_backend_open_or_close(Some("textDocument/didOpen"), &first, &view)
            .await
            .is_none(),
        "the first open is forwarded as it came"
    );
    assert!(backend.open_files.read().await.contains(uri));
    for (message, name) in [
        (open(uri, Some("two"), Some(5)), "with text and version"),
        (open(uri, None, None), "without either"),
    ] {
        let flow =
            intercept_backend_open_or_close(Some("textDocument/didOpen"), &message, &view).await;
        assert!(matches!(flow, Some(Flow::Next)), "{name}");
    }
    let hover = json!({ "method": "textDocument/hover", "params": {} });
    assert!(
        intercept_backend_open_or_close(Some("textDocument/hover"), &hover, &view)
            .await
            .is_none()
    );
    let close =
        json!({ "method": "textDocument/didClose", "params": { "textDocument": { "uri": uri } } });
    assert!(
        intercept_backend_open_or_close(Some("textDocument/didClose"), &close, &view)
            .await
            .is_none()
    );
    assert!(!backend.open_files.read().await.contains(uri));
    // Closed documents are opened afresh; a close for one never opened changes nothing.
    assert!(
        intercept_backend_open_or_close(Some("textDocument/didOpen"), &first, &view)
            .await
            .is_none()
    );
    assert!(backend.open_files.read().await.contains(uri));
    let stray = json!({ "method": "textDocument/didClose", "params": {} });
    assert!(
        intercept_backend_open_or_close(Some("textDocument/didClose"), &stray, &view)
            .await
            .is_none()
    );
    assert!(backend.open_files.read().await.contains(uri));

    let change = |version, text| {
        json!({
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }]
            }
        })
    };
    assert_eq!(
        received(&backend, 9001).await,
        json!([
            { "method": "initialized", "params": {} },
            change(5, "two"),
            change(2, ""),
        ])
    );
    drop(out);
    assert!(rx.recv().await.is_err());
    view.retire().await;
}
