/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::{collect_functions, session_symbol_line};
use std::path::Path;

fn range(sl: u32, sc: u32, el: u32, ec: u32) -> serde_json::Value {
    serde_json::json!({
        "start": { "line": sl, "character": sc },
        "end": { "line": el, "character": ec }
    })
}

#[test]
fn callable_symbols_are_collected_with_their_ranges_and_name_positions() {
    let symbols = serde_json::json!([
        {
            "name": "f", "kind": 12, "range": range(0, 0, 4, 1),
            "selectionRange": range(0, 3, 0, 4),
            "children": [
                { "name": "inner", "kind": 12, "range": range(1, 2, 2, 3), "selectionRange": range(1, 11, 1, 16) }
            ]
        },
        {
            "name": "S", "kind": 23, "range": range(5, 0, 9, 1),
            "children": [
                { "name": "m", "kind": 6, "range": range(6, 4, 8, 5), "selectionRange": range(6, 7, 6, 8) }
            ]
        },
        { "name": "arrow", "kind": 13, "detail": "(x) => 1", "range": range(10, 0, 10, 20), "selectionRange": range(10, 6, 10, 11) },
        { "name": "viaFunction", "kind": 14, "detail": "function", "range": range(11, 0, 11, 9) },
        { "name": "plain", "kind": 13, "detail": "number", "range": range(12, 0, 12, 9) },
        { "name": "bare", "kind": 13, "range": range(12, 0, 12, 9) },
        { "name": "ctor", "kind": 9, "location": { "uri": "file:///x.ts", "range": range(13, 1, 14, 2) } },
        { "name": "noRange", "kind": 12 },
        { "kind": 12, "range": range(20, 0, 21, 0) }
    ]);
    let mut out = Vec::new();
    collect_functions(symbols.as_array().unwrap(), &mut out);
    let want: Vec<(String, u32, u32, u32, u32)> = [
        ("f", 1, 5, 1, 4),
        ("inner", 2, 3, 2, 12),
        ("m", 7, 9, 7, 8),
        ("arrow", 11, 11, 11, 7),
        ("viaFunction", 12, 12, 12, 1),
        ("ctor", 14, 15, 14, 2),
        ("", 21, 22, 21, 1),
    ]
    .iter()
    .map(|(n, a, b, c, d)| (n.to_string(), *a, *b, *c, *d))
    .collect();
    assert_eq!(out, want);

    let mut none = Vec::new();
    collect_functions(&[], &mut none);
    assert!(none.is_empty());
}

#[tokio::test]
async fn without_a_session_there_is_no_symbol_line() {
    assert_eq!(
        session_symbol_line(None, Path::new("/x/test_y.py"), "test_y").await,
        None
    );
}
