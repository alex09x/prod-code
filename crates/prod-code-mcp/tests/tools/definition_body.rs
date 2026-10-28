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
async fn code_definition_symbol_fallback_explains_unreadable_worktree_bodies() {
    let base = workspace();
    let linked = rust_workspace("pub fn target() {}\n");
    let file = linked.path("src/lib.rs");
    let script = Script {
        lsp: Arc::new(|method, _| {
            if method == "textDocument/definition" {
                serde_json::json!([])
            } else {
                serde_json::Value::Null
            }
        }),
        read_file: None,
        ..Script::default()
    };

    for body in [true, false] {
        let result = execute_tool(
            mock_gateway(script.clone()).await,
            &base.root(),
            "code_definition",
            serde_json::json!({
                "path": file.to_string_lossy(),
                "symbol": "target",
                "body": body
            }),
        )
        .await
        .expect("the definition call returns its path guidance");
        let text = text_of(&result);
        assert!(text.contains("Definition:"), "{text}");
        assert!(
            text.contains("outside the workspace synced to this gateway"),
            "{text}"
        );
        assert!(text.contains("sync or select the worktree"), "{text}");
    }
}
