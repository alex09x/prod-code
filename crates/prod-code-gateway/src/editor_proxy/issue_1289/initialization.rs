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
use prod_code_engine_generic::GenericLspConfig;

async fn capture(engine: &str, defaults: Option<Value>, editor: Option<Value>) -> Value {
    let native = engine == "typescript-native";
    let config = GenericLspConfig {
        command: "controlled-language-server".into(),
        args: if native {
            vec!["--lsp".into(), "--stdio".into()]
        } else {
            vec!["--stdio".into()]
        },
        initialization_options: defaults,
        ..Default::default()
    };
    let command = ServerCommand::from_generic(if native { "typescript" } else { engine }, config);
    let mut session = Session::start(command.initialization_options, WAIT).await;
    session.release();
    let mut params = json!({"processId":123, "rootUri":"file:///client", "capabilities":{}});
    if let Some(editor) = editor {
        params["initializationOptions"] = editor;
    }
    session
        .send(json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":params}))
        .await;
    session.receive().await;
    let captured: Value = serde_json::from_str(
        &std::fs::read_to_string(session.root.path().join("initialize.json")).unwrap(),
    )
    .unwrap();
    assert!(captured["params"]["processId"].is_null());
    assert_eq!(
        captured["params"]["rootUri"],
        format!("file://{}", session.root.path().display())
    );
    session.disconnect().await;
    captured["params"]["initializationOptions"].clone()
}

#[tokio::test]
async fn editor_proxy_issue_1289_typescript_language_server_path_reaches_initialize() {
    let defaults = json!({"tsserver":{"path":"/server/npm/typescript/lib"},
        "preferences":{"includeInlayParameterNameHints":"none"}});
    let editor = json!({"preferences":{"includeInlayParameterNameHints":"all"},
        "tsserver":{"logVerbosity":"verbose"}, "locale":"fr"});
    let options = capture("typescript", Some(defaults.clone()), Some(editor.clone())).await;
    assert_eq!(options["tsserver"]["path"], "/server/npm/typescript/lib");
    assert_eq!(options["preferences"], editor["preferences"]);
    assert_eq!(options["tsserver"]["logVerbosity"], "verbose");
    assert_eq!(options["locale"], "fr");
    for omitted in [None, Some(Value::Null)] {
        assert_eq!(
            capture("typescript", Some(defaults.clone()), omitted).await,
            json!({"tsserver":{"path":"/server/npm/typescript/lib"}})
        );
    }
    let explicit = json!({"tsserver":{"path":"/server/workspace/typescript/lib"},
        "preferences":{"includeInlayParameterNameHints":"literals"}});
    assert_eq!(
        capture("typescript", Some(defaults), Some(explicit.clone())).await,
        explicit
    );
}

#[tokio::test]
async fn editor_proxy_issue_1289_vtsls_path_preserves_editor_preferences() {
    let defaults = json!({"typescript":{"tsserver":{"path":"/server/npm/typescript/lib",
        "maxTsServerMemory":4096}, "preferences":{"includeInlayParameterNameHints":"none"}},
        "vtsls":{"autoUseWorkspaceTsdk":true}});
    let editor = json!({"typescript":{"tsserver":{"maxTsServerMemory":8192},
        "preferences":{"includeInlayParameterNameHints":"all"}},
        "vtsls":{"autoUseWorkspaceTsdk":false}, "custom":[1,2]});
    let mut expected = editor.clone();
    expected["typescript"]["tsserver"]["path"] = json!("/server/npm/typescript/lib");
    assert_eq!(
        capture("typescript", Some(defaults.clone()), Some(editor)).await,
        expected
    );
    assert_eq!(
        capture("typescript", Some(defaults.clone()), None).await,
        json!({"typescript":{"tsserver":{"path":"/server/npm/typescript/lib"}}})
    );
    expected["typescript"]["tsserver"]["path"] = json!("/server/workspace/lib");
    assert_eq!(
        capture("typescript", Some(defaults), Some(expected.clone())).await,
        expected
    );
}

#[tokio::test]
async fn editor_proxy_issue_1289_native_unrelated_and_python_options_remain_compatible() {
    let editor = json!({"preferences":{"includeInlayParameterNameHints":"all"}, "custom":42});
    let native_defaults = Some(json!({"tsserver":{"path":"/server/unused-sdk"}}));
    assert_eq!(
        capture(
            "typescript-native",
            native_defaults.clone(),
            Some(editor.clone())
        )
        .await,
        editor
    );
    assert_eq!(
        capture("typescript-native", native_defaults, None).await,
        Value::Null
    );
    assert_eq!(
        capture(
            "java",
            Some(json!({"java":{"autobuild":{"enabled":false}}})),
            Some(editor.clone())
        )
        .await,
        editor
    );
    let python = json!({"python":{"analysis":{"stubPath":"/server/typings"}}});
    let editor =
        json!({"python":{"analysis":{"stubPath":"/client/typings", "typeCheckingMode":"strict"}}});
    assert_eq!(
        capture("python", Some(python), Some(editor)).await,
        json!({"python":{"analysis":{"stubPath":"/server/typings", "typeCheckingMode":"strict"}}})
    );
}
