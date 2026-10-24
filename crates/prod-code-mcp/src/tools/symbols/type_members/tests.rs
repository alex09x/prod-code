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

#[tokio::test]
async fn rust_module_is_not_treated_as_a_type_owning_methods() {
    use prod_code_testkit::{ScriptedGateway, Workspace, answers};
    let ws = Workspace::new(&[("sample.rs", "pub mod Store {}\n")]);
    let path = ws.path("sample.rs");
    let gw = ScriptedGateway::start(move |method, _| {
        if method == "workspace/symbol" {
            json!([answers::symbol("Store", 2, &path, 1, 9)])
        } else {
            json!(null)
        }
    })
    .await;
    let result = type_members(gw.addr(), &ws.root(), &["Store"], "value", None)
        .await
        .unwrap();
    assert_eq!(result.len(), 0);
    assert_eq!(gw.calls(), 1);
}

#[tokio::test]
async fn absent_owner_has_no_members_without_asking_an_analyzer() {
    let ws = prod_code_testkit::Workspace::new(&[]);
    let members = type_members(
        "127.0.0.1:1".parse().unwrap(),
        &ws.root(),
        &[],
        "value",
        None,
    )
    .await
    .unwrap();
    assert_eq!(members.len(), 0);
}

#[test]
fn member_coordinates_prefer_selection_then_range_then_location() {
    for (symbol, expected) in [
        (
            json!({"name":"雪","kind":6,"selectionRange":{"start":{"line":2,"character":3}},"range":{"start":{"line":0,"character":0}}}),
            ("雪", 6, 3, 4),
        ),
        (
            json!({"name":"value","range":{"start":{"line":0,"character":0}}}),
            ("value", 0, 1, 1),
        ),
        (
            json!({"name":"value","kind":8,"location":{"range":{"start":{"line":4,"character":7}}}}),
            ("value", 8, 5, 8),
        ),
    ] {
        assert_eq!(
            member_at(&symbol).unwrap(),
            (expected.0.into(), expected.1, expected.2, expected.3)
        );
    }
    for (symbol, expected) in [
        (
            json!({"name":"value"}),
            "malformed LSP member symbol: missing selection range start",
        ),
        (
            json!({"range":{"start":{"line":0,"character":0}}}),
            "malformed LSP member symbol: missing name",
        ),
        (
            json!({"name":"value","range":{"start":{"line":-1,"character":0}}}),
            "malformed LSP member symbol `value`: `line` must be a non-negative integer",
        ),
    ] {
        assert_eq!(member_at(&symbol).unwrap_err().to_string(), expected);
    }
}

#[test]
fn member_collection_distinguishes_rust_locals_from_python_attributes() {
    let root = Path::new("/ws");
    let symbols = json!([{
        "name":"Store", "kind":23, "children":[
            {"name":"value","kind":13,"range":{"start":{"line":1,"character":4}}},
            {"name":"value","kind":8,"range":{"start":{"line":2,"character":4}}}
        ]
    }]);
    for (path, expected) in [
        ("/ws/sample.rs", vec![("value".into(), 8, 3, 5)]),
        (
            "/ws/sample.py",
            vec![("value".into(), 13, 2, 5), ("value".into(), 8, 3, 5)],
        ),
    ] {
        let mut hits = Vec::new();
        collect_members(
            &symbols,
            root,
            Path::new(path),
            &["Store"],
            "value",
            &[],
            &mut hits,
        )
        .unwrap();
        assert_eq!(hits, expected);
    }
    let mut hits = Vec::new();
    collect_members(
        &json!([{"name":"Store","kind":12,"children":[{"name":"value","kind":13}]}]),
        root,
        Path::new("/ws/sample.py"),
        &["Store"],
        "value",
        &[],
        &mut hits,
    )
    .unwrap();
    assert_eq!(hits, vec![]);
    collect_members(
        &json!(null),
        root,
        Path::new("/ws/sample.rs"),
        &[],
        "value",
        &[],
        &mut hits,
    )
    .unwrap();
    assert_eq!(hits, vec![]);
}
