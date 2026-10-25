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
