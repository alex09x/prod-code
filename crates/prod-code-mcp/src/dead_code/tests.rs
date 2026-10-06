/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::{collect, in_trait_impl, is_exported, is_test_path, reference_count};
use anyhow::anyhow;

#[test]
fn export_detection() {
    assert!(is_exported("rust", "f", "pub fn f() {}"));
    assert!(!is_exported("rust", "f", "fn f() {}"));
    assert!(is_exported("go", "Compute", "func Compute() {}"));
    assert!(!is_exported("go", "compute", "func compute() {}"));
    assert!(is_exported("typescript", "f", "export function f() {}"));
    assert!(is_exported("swift", "f", "public func f() {}"));
    assert!(is_test_path("go", "pkg/a_test.go"));
    assert!(in_trait_impl("impl Shape for Circle"));
    assert!(!in_trait_impl("impl Circle"));
    assert!(!is_test_path("rust", "src/lib.rs"));
}

#[test]
fn a_malformed_symbol_fails_the_file_instead_of_being_skipped() {
    let at = |line: serde_json::Value| {
        serde_json::json!({ "name": "f", "kind": 12,
            "selectionRange": { "start": { "line": line, "character": 3 } } })
    };
    let mut out = Vec::new();
    assert_eq!(collect(&[at(serde_json::json!(2))], &mut out), Ok(()));
    assert_eq!(out, vec![("f".to_string(), "function".to_string(), 3, 4)]);
    for bad in [
        serde_json::json!(null),
        serde_json::json!({ "kind": 12 }),
        serde_json::json!({ "name": "", "kind": 12 }),
        serde_json::json!({ "name": "f", "kind": 0 }),
        serde_json::json!({ "name": "f", "kind": 99 }),
        serde_json::json!({ "name": "f", "kind": "12" }),
        serde_json::json!({ "name": "f", "kind": 12 }),
        serde_json::json!({ "name": "f", "kind": 12, "containerName": 3 }),
        at(serde_json::json!(-1)),
        at(serde_json::json!(4_294_967_296u64)),
        serde_json::json!({ "name": "S", "kind": 23, "children": "f",
            "selectionRange": { "start": { "line": 0, "character": 0 } } }),
        serde_json::json!({ "name": "m", "kind": 2, "children": [at(serde_json::json!("2"))] }),
    ] {
        let error = collect(std::slice::from_ref(&bad), &mut Vec::new()).unwrap_err();
        assert!(error.contains("cannot read"), "{bad}: {error}");
    }
    assert_eq!(collect(&[], &mut Vec::new()), Ok(()));
}

#[test]
fn only_a_list_counts_references() {
    assert_eq!(reference_count(Ok(serde_json::json!([]))), Ok(0));
    assert_eq!(reference_count(Ok(serde_json::json!([{}, {}]))), Ok(2));
    assert!(
        reference_count(Ok(serde_json::Value::Null))
            .unwrap_err()
            .contains("null")
    );
    assert!(
        reference_count(Ok(serde_json::json!({ "uri": "x" })))
            .unwrap_err()
            .contains("cannot read")
    );
    assert!(
        reference_count(Err(anyhow!("textDocument/references failed: boom")))
            .unwrap_err()
            .contains("boom")
    );
}
