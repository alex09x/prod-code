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
use serde_json::json;

#[test]
fn stable_ids_and_range_defaults_cover_malformed_and_unicode_inputs() {
    for (action, expected) in [
        (json!({}), "3:action"),
        (json!({"title":4}), "3:action"),
        (json!({"title":""}), "3:"),
        (
            json!({"title":"É😀 Fix--ABC\r\nOne Two Three Four Five Six"}),
            "3:fix_abc_one_two_three_four",
        ),
        // The server's own id (Rust Intelligence: rust-analyzer's assist id), with no index.
        (
            json!({"title":"Extract into variable","data":{"id":"extract_variable"}}),
            "extract_variable",
        ),
        (json!({"title":"Fix it","data":{"id":""}}), "3:fix_it"),
        (json!({"title":"Fix it","data":{"id":7}}), "3:fix_it"),
    ] {
        assert_eq!(code_action_id(3, &action), expected);
    }
    for (range, start, end) in [
        (json!({}), 0, 0),
        (json!({"start":{"line":-1},"end":{"line":"2"}}), 0, 0),
        (
            json!({"start":{"line":4},"end":{"line":u64::MAX}}),
            4,
            u64::MAX,
        ),
    ] {
        assert_eq!(range_line(&range, false), start);
        assert_eq!(range_line(&range, true), end);
    }
}

#[tokio::test]
async fn listing_and_application_select_exact_edits_resolve_lazy_edits_and_report_refusals() {
    let temp = tempfile::tempdir().unwrap();
    let edit = json!({"changes":{"file:///sample.py":[]}});
    let engine = crate::lsp::test_support::server(
        temp.path(),
        json!({
            "textDocument/diagnostic":{"result":{"kind":"full","items":[]}},
            "textDocument/codeAction":{"result":[
                {"title":"Direct fix","kind":"quickfix","edit":edit},
                {"title":"Lazy fix","edit":null},
                {"title":"Command fix","command":"fix","arguments":[1]},
                {"title":"Empty fix"},
                {"command":{"command":"fix"}}
            ]},
            "codeAction/resolve":{"result":{}},
            "workspace/executeCommand":{"result":null}
        }),
    )
    .await;
    let managed = ManagedLsp::Generic(&engine);
    assert_eq!(
        lsp_code_actions(&managed, "prodCode/assists", json!({}))
            .await
            .unwrap(),
        json!([
            {"id":"0:direct_fix","kind":"quickfix","label":"Direct fix"},
            {"id":"1:lazy_fix","kind":"action","label":"Lazy fix"},
            {"id":"2:command_fix","kind":"command","label":"Command fix"},
            {"id":"3:empty_fix","kind":"action","label":"Empty fix"},
            {"id":"4:action","kind":"command","label":""}
        ])
    );
    for id in ["0:direct_fix", "99:direct_fix", "direct_fix"] {
        assert_eq!(
            lsp_code_actions(&managed, "prodCode/applyAssist", json!({"id":id}))
                .await
                .unwrap(),
            edit
        );
    }
    for (id, expected) in [
        (
            "missing",
            "no code action `missing` at this position (available: 0:direct_fix, 1:lazy_fix, 2:command_fix, 3:empty_fix, 4:action)",
        ),
        (
            "1:lazy_fix",
            "code action `Lazy fix` carries neither an edit nor a command",
        ),
        (
            "3:empty_fix",
            "code action `Empty fix` carries neither an edit nor a command",
        ),
    ] {
        assert_eq!(
            lsp_code_actions(&managed, "prodCode/applyAssist", json!({"id":id}))
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
    }
    let temp = tempfile::tempdir().unwrap();
    let resolved = crate::lsp::test_support::server(
        temp.path(),
        json!({
            "textDocument/diagnostic":{"result":{"kind":"full","items":[]}},
            "textDocument/codeAction":{"result":[{"title":"Lazy fix"}]},
            "codeAction/resolve":{"result":{"edit":edit}}
        }),
    )
    .await;
    assert_eq!(
        lsp_code_actions(
            &ManagedLsp::Generic(&resolved),
            "prodCode/applyAssist",
            json!({"id":"0:lazy_fix"})
        )
        .await
        .unwrap(),
        edit
    );
}

/// Actions that share a server id (one `auto_import` per import path) are listed with their
/// subtype, applied by it, and refused as ambiguous without it.
#[tokio::test]
async fn actions_sharing_a_server_id_are_chosen_by_subtype() {
    let temp = tempfile::tempdir().unwrap();
    let first = json!({"changes":{"file:///a.rs":[{"newText":"use a::Map;\n"}]}});
    let second = json!({"changes":{"file:///a.rs":[{"newText":"use b::Map;\n"}]}});
    let engine = crate::lsp::test_support::server(
        temp.path(),
        json!({
            "textDocument/diagnostic":{"result":{"kind":"full","items":[]}},
            "textDocument/codeAction":{"result":[
                {"title":"Import `a::Map`","kind":"quickfix","edit":first,"data":{"id":"auto_import"}},
                {"title":"Import `b::Map`","kind":"quickfix","edit":second,"data":{"id":"auto_import"}},
                {"title":"Add explicit type","kind":"refactor.rewrite","edit":first,"data":{"id":"add_explicit_type"}}
            ]}
        }),
    )
    .await;
    let managed = ManagedLsp::Generic(&engine);
    assert_eq!(
        lsp_code_actions(&managed, "prodCode/assists", json!({}))
            .await
            .unwrap(),
        json!([
            {"id":"auto_import","kind":"quickfix","label":"Import `a::Map`","subtype":0},
            {"id":"auto_import","kind":"quickfix","label":"Import `b::Map`","subtype":1},
            {"id":"add_explicit_type","kind":"refactor.rewrite","label":"Add explicit type"}
        ])
    );
    for (subtype, edit) in [(0, &first), (1, &second)] {
        assert_eq!(
            &lsp_code_actions(
                &managed,
                "prodCode/applyAssist",
                json!({"id":"auto_import","subtype":subtype})
            )
            .await
            .unwrap(),
            edit
        );
    }
    let ambiguous = lsp_code_actions(
        &managed,
        "prodCode/applyAssist",
        json!({"id":"auto_import"}),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        ambiguous.contains("ambiguous") && ambiguous.contains("1: Import `b::Map`"),
        "{ambiguous}"
    );
    let missing = lsp_code_actions(
        &managed,
        "prodCode/applyAssist",
        json!({"id":"auto_import","subtype":2}),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(missing.contains("no subtype 2"), "{missing}");
}
