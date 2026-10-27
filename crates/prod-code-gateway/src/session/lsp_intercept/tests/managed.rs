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

#[tokio::test]
async fn managed_assists_return_translated_edits_and_exact_refusals() {
    let (dir, mut view, out, mut rx) = bare_session().await;
    let edit = json!({"changes":{"file:///server/ws/sample.py":[{
        "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},
        "newText":"fixed"
    }]}});
    let engine = Arc::new(crate::lsp::test_support::server(dir.path(), json!({
        "textDocument/diagnostic":{"result":{"kind":"full","items":[]}},
        "textDocument/codeAction":{"result":[{"title":"Fix sample","kind":"quickfix","edit":edit}]}
    })).await);
    *engine.capabilities.write().await = Some(json!({
        "hoverProvider":false,
        "textDocumentSync":{"save":true}
    }));
    let workspace = Arc::new(workspace::SharedWorkspace::new(
        view.workspace.root.clone(),
        "python".into(),
        None,
        None,
        Some(engine),
        None,
    ));
    view.workspace = workspace;
    let translator = PathTranslator::new("/client/ws", "/server/ws");
    assert!(matches!(
        intercept_lifecycle_lsp(
            Some("initialize"),
            &Some(json!(7)),
            &view,
            &out,
            &translator
        )
        .await,
        Some(Flow::Next)
    ));
    assert_eq!(
        next_payload(&mut rx).await["result"]["capabilities"],
        json!({
            "hoverProvider":false,"textDocumentSync":{"openClose":true,"change":1,"save":true}
        })
    );
    assert!(
        fallback_empty_lsp(&Some(json!(99)), &view, &out)
            .await
            .is_none()
    );
    let cases = [
        (
            "prodCode/assists",
            json!({}),
            json!({"result":[{"id":"0:fix_sample","kind":"quickfix","label":"Fix sample"}]}),
        ),
        (
            "prodCode/applyAssist",
            json!({"id":"0:fix_sample"}),
            json!({"result":{
                "changes":{"file:///client/ws/sample.py":edit["changes"]["file:///server/ws/sample.py"]}
            }}),
        ),
        (
            "prodCode/applyAssist",
            json!({"id":"missing"}),
            json!({"error":{
                "code":-32602,"message":"no code action `missing` at this position (available: 0:fix_sample)"
            }}),
        ),
    ];
    for (index, (method, params, mut expected)) in cases.into_iter().enumerate() {
        let id = json!(index);
        let request = json!({"params":params});
        assert!(matches!(
            intercept_assists_for_managed(
                Some(method),
                &Some(id.clone()),
                &request,
                &view,
                &out,
                &translator
            ),
            Some(Flow::Next)
        ));
        expected["jsonrpc"] = json!("2.0");
        expected["id"] = id;
        let actual = tokio::time::timeout(std::time::Duration::from_secs(5), next_payload(&mut rx))
            .await
            .unwrap();
        assert_eq!(actual, expected);
    }
    // Absent params still lists at the default position; notifications must not launch work.
    assert!(matches!(
        intercept_assists_for_managed(
            Some("prodCode/assists"),
            &Some(json!(12)),
            &json!({}),
            &view,
            &out,
            &translator
        ),
        Some(Flow::Next)
    ));
    assert_eq!(
        next_payload(&mut rx).await,
        json!({"jsonrpc":"2.0","id":12,"result":[{"id":"0:fix_sample","kind":"quickfix","label":"Fix sample"}]})
    );
    assert!(
        intercept_assists_for_managed(
            Some("prodCode/assists"),
            &None,
            &json!({}),
            &view,
            &out,
            &translator
        )
        .is_none()
    );
    drop(out);
    assert!(rx.recv().await.is_err());
    view.retire().await;
}

/// #1518: a document notification the language server refuses ends the session with the
/// reason, rather than leaving later queries to answer from a document it never took. One it
/// takes goes on as before.
#[tokio::test]
async fn a_refused_document_notification_ends_the_session() {
    let (dir, mut view, out, mut rx) = bare_session().await;
    let engine = Arc::new(crate::lsp::test_support::server(dir.path(), json!({})).await);
    view.workspace = Arc::new(workspace::SharedWorkspace::new(
        view.workspace.root.clone(),
        "python".into(),
        None,
        None,
        Some(engine),
        None,
    ));
    let translator = PathTranslator::new("/client/ws", "/server/ws");
    let uri = url::Url::from_file_path(view.workspace.root.join("a.py"))
        .unwrap()
        .to_string();
    let open = |document: Value| json!({ "method": "textDocument/didOpen", "params": { "textDocument": document } });

    let taken =
        open(json!({ "uri": uri, "languageId": "python", "version": 1, "text": "x = 1\n" }));
    assert!(matches!(
        handle_generic_lsp(
            Some("textDocument/didOpen"),
            &None,
            &taken,
            &view,
            &out,
            &translator
        )
        .await,
        Some(Flow::Next)
    ));

    // An open with no text is refused by the engine.
    let refused =
        open(json!({ "uri": uri.replace("a.py", "b.py"), "languageId": "python", "version": 1 }));
    assert!(matches!(
        handle_generic_lsp(
            Some("textDocument/didOpen"),
            &None,
            &refused,
            &view,
            &out,
            &translator
        )
        .await,
        Some(Flow::Stop)
    ));
    match rx.recv().await.unwrap().message {
        WireMessage::Disconnect { reason } => {
            assert!(
                reason.contains("refused `textDocument/didOpen`"),
                "{reason}"
            )
        }
        other => panic!("expected a disconnect, got {other:?}"),
    }
}
