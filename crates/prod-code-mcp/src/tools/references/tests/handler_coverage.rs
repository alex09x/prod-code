/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::tools::references::*;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::json;

#[tokio::test]
async fn references_limit_and_empty_position_have_exact_explanations() {
    let ws = Workspace::new(&[("sample.rs", "fn value() {}\r\nfn call() { value(); }\r\n")]);
    let file = ws.path("unreadable.rs");
    let uri = answers::uri(&file);
    let gw = ScriptedGateway::start(move |method, _| {
        if method == "textDocument/references" {
            answers::locations(&file, &[(1, 4), (2, 13)])
        } else {
            json!(null)
        }
    })
    .await;
    let result = handle_references(
        gw.addr(),
        &ws.root(),
        &json!({"path":"sample.rs","line":1,"character":4,"limit":1,"symbol":"value"}),
    )
    .await
    .unwrap();
    assert!(!result.is_error);
    let crate::protocol::McpContentItem::Text { text } = &result.content[0];
    assert_eq!(
        text,
        &format!(
            "Found 2 reference(s):\n  • {uri}:1:4\n  ... (display capped at first 1 of 2 references; pass `limit: 0` to display all)\n\n💡 Refactoring Tip: For workspace-wide renames or signature updates to `value`, prefer `code_rename` or `code_change_signature`. If the specialized tool reports unsupported or blocked, fallback to manual edits and inspect diffs carefully."
        )
    );
    let error = handle_references(
        gw.addr(),
        &ws.root(),
        &json!({"path":"sample.rs","line":1,"character":12}),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "sample.rs:1:12 is on no name; the line reads `fn value() {}`. Give the position of a use or a declaration, or pass `symbol`"
    );
}

#[test]
fn definition_shapes_distinguish_same_other_and_external_files() {
    let root = std::path::Path::new("/ws");
    for (reply, same, other) in [
        (json!(null), false, false),
        (json!({"uri":"file:///ws/decl.rs"}), true, false),
        (json!([{"targetUri":"file:///ws/other.rs"}]), false, true),
        (json!({"uri":"file:///external/decl.rs"}), false, false),
        (json!({}), false, false),
    ] {
        assert_eq!(definition_in_file(&reply, "/ws/decl.rs"), same);
        assert_eq!(
            definition_in_other_workspace_file(&reply, root, "/ws/decl.rs"),
            other
        );
    }
    for (caller, decl, imported, expected) in [
        (
            "/ws/Sources/A/Sub/c.swift",
            "/ws/Sources/A/d.swift",
            false,
            true,
        ),
        ("/ws/Tests/A/c.swift", "/ws/Sources/A/d.swift", false, false),
        ("/ws/c.swift", "/ws/d.swift", false, true),
        ("/a/c.swift", "/b/d.swift", true, true),
    ] {
        assert_eq!(
            swift_shares_or_imports_target(
                std::path::Path::new(caller),
                std::path::Path::new(decl),
                imported
            ),
            expected
        );
    }
}
