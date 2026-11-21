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

/// One `remove_unnecessary_mut` fix per `unused_mut` warning: one id, one title, different
/// ranges and edits.
fn mut_fix(line: u64) -> serde_json::Value {
    let range = json!({"start":{"line":line,"character":8},"end":{"line":line,"character":12}});
    json!({
        "title": "Remove unnecessary `mut`",
        "kind": "quickfix",
        "diagnostics": [{"range": range, "code": "unused_mut", "message": "variable does not need to be mutable"}],
        "edit": {"changes":{"file:///a.rs":[{"range": range, "newText": ""}]}},
        "data": {"id": "remove_unnecessary_mut"}
    })
}

/// Actions that share a server id are listed by a variant id derived from what each one does
/// (title, edit, the diagnostics it fixes). Applying re-queries the server: a reordered list
/// still applies the variant asked for; a variant that is gone is refused rather than replaced
/// by the one left, even when the other has the same title; a plain id that is shared is
/// ambiguous; an exact copy of an action is one action; and a subtype selects nothing here.
#[tokio::test]
async fn actions_sharing_a_server_id_are_chosen_by_what_they_change() {
    let temp = tempfile::tempdir().unwrap();
    let (first, second) = (mut_fix(3), mut_fix(7));
    let net_http = json!({"title":"Import `net_http::Map`","kind":"quickfix","data":{"id":"auto_import"},
        "edit":{"changes":{"file:///a.rs":[{"newText":"use net_http::Map;\n"}]}}});
    let net_dot_http = json!({"title":"Import `net::http::Map`","kind":"quickfix","data":{"id":"auto_import"},
        "edit":{"changes":{"file:///a.rs":[{"newText":"use net::http::Map;\n"}]}}});
    let engine = crate::lsp::test_support::server(
        temp.path(),
        json!({
            "textDocument/diagnostic":{"result":{"kind":"full","items":[]}},
            "textDocument/codeAction":[
                {"result":[first, second, net_http, net_dot_http, first]},
                {"result":[second, first]},
                {"result":[first]},
                {"result":[first, second]},
                {"result":[first, second]}
            ]
        }),
    )
    .await;
    let managed = ManagedLsp::Generic(&engine);
    let apply =
        |params: serde_json::Value| lsp_code_actions(&managed, "prodCode/applyAssist", params);
    let listed = lsp_code_actions(&managed, "prodCode/assists", json!({}))
        .await
        .unwrap();
    let ids: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    // The exact copy of the first fix is one action; the other four are each their own.
    assert_eq!(ids.len(), 4, "{listed}");
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        4,
        "every variant has its own selector: {listed}"
    );
    assert!(
        ids.iter()
            .all(|id| id.starts_with("remove_unnecessary_mut:") || id.starts_with("auto_import:")),
        "{listed}"
    );
    let (first_id, second_id) = (ids[0].to_string(), ids[1].to_string());
    // Reordered: the variant asked for, not the one now first.
    assert_eq!(
        apply(json!({"id": second_id})).await.unwrap(),
        second["edit"]
    );
    // Gone: refused, although the fix left has the same id and title.
    let gone = apply(json!({"id": second_id}))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        gone.contains(&format!("no code action `{second_id}`")),
        "{gone}"
    );
    let ambiguous = apply(json!({"id":"remove_unnecessary_mut"}))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        ambiguous.contains("ambiguous") && ambiguous.contains(&second_id),
        "{ambiguous}"
    );
    let subtype = apply(json!({"id": first_id, "subtype": 0}))
        .await
        .unwrap_err()
        .to_string();
    assert!(subtype.contains("a subtype does not select"), "{subtype}");
}
