/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::parse_workspace_symbol_hits;
use serde_json::json;

fn symbol(uri: &str) -> serde_json::Value {
    json!({
        "name": "Thing",
        "kind": 5,
        "location": { "uri": uri, "range": {
            "start": { "line": 2, "character": 4 },
            "end": { "line": 2, "character": 9 }
        } }
    })
}

/// An exact-name hit in a virtual document, such as a class inside a jar, is skipped and the
/// file hit after it is still found (#1437).
#[test]
fn a_virtual_document_hit_is_skipped_before_a_file_hit() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("src/thing.rs");
    let file_uri = url::Url::from_file_path(&file).unwrap().to_string();
    let response = json!([
        symbol("jar:file:///virtual/archive.jar!/Thing.class"),
        symbol(&file_uri)
    ]);
    let hits = parse_workspace_symbol_hits(root.path(), "Thing", &response, 10)
        .expect("a virtual document does not abort the search");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].path, file);
    assert_eq!(
        (hits[0].line, hits[0].name.as_str()),
        (3, "Thing"),
        "1-based line"
    );
}

/// Text that is no URI at all, on an exact-name hit, is still a malformed answer.
#[test]
fn a_malformed_uri_on_an_exact_hit_still_fails() {
    let root = tempfile::tempdir().unwrap();
    let response = json!([symbol("not a uri")]);
    let error = parse_workspace_symbol_hits(root.path(), "Thing", &response, 10)
        .expect_err("malformed coordinates are refused");
    assert!(
        error.to_string().contains("invalid location URI"),
        "{error}"
    );
}
