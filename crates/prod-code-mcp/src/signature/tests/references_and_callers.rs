/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;

use crate::signature::references::{parse_locations, references, unreported_callers, writes_call};

/// A file that calls the function is found though no analyzer reported it; the declaring
/// file, a file already checked, another language, and a longer name that starts the same
/// are not (#294). C and C++ are one family: a C function is called from C++ too.
#[test]
fn files_that_call_the_function_but_were_not_reported_are_found() {
    let ws = prod_code_testkit::Workspace::new(&[
        (
            "Sources/Shop/Pricing.swift",
            "func price(qty: Int) -> Int { qty }\n",
        ),
        ("Sources/Shop/main.swift", "print(price (qty: 3))\n"),
        ("Sources/Shop/Checked.swift", "print(price(qty: 4))\n"),
        (
            "Sources/Shop/Other.swift",
            "let a = prices(1) + unit_price(2)\n",
        ),
        ("tools/notes.py", "price(3)\n"),
        ("src/pricing.c", "int price(int qty) { return qty; }\n"),
        ("src/app.cpp", "int main() { return price(3); }\n"),
        ("src/lib.rs", "fn f() { price(3); }\n"),
    ]);
    let root = ws.root();
    assert_eq!(
        unreported_callers(
            &root,
            &root.join("Sources/Shop/Pricing.swift"),
            "price",
            &[root.join("Sources/Shop/Checked.swift")],
        ),
        vec![root.join("Sources/Shop/main.swift")]
    );
    assert_eq!(
        unreported_callers(&root, &root.join("src/pricing.c"), "price", &[]),
        vec![root.join("src/app.cpp")]
    );
    assert!(
        unreported_callers(&root, &root.join("src/lib.rs"), "price", &[]).is_empty(),
        "Rust is not searched"
    );
    assert!(writes_call("x = price(1)", "price"));
    assert!(!writes_call("x = price", "price"));
}

/// A language server that answers "no references" while it reads the project is asked
/// again before the answer is believed; rust-analyzer is believed at once (#284).
#[tokio::test]
async fn an_empty_answer_from_a_cold_server_is_asked_again() {
    let ws = prod_code_testkit::Workspace::new(&[
        ("pricing.py", "def price(qty):\n    return qty\n"),
        ("lib.rs", "pub fn price() {}\n"),
    ]);
    let root_buf = ws.root();
    let root = root_buf.as_path();
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = std::sync::Arc::clone(&asked);
    let caller = root.join("cart.py");
    let gateway = prod_code_testkit::ScriptedGateway::start(move |method, _| {
        if method != "textDocument/references" {
            return serde_json::Value::Null;
        }
        // Empty for the first two questions, as a server still indexing answers.
        if count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
            return serde_json::json!([]);
        }
        serde_json::json!([{
            "uri": url::Url::from_file_path(&caller).unwrap().to_string(),
            "range": { "start": { "line": 4, "character": 11 }, "end": { "line": 4, "character": 16 } }
        }])
    })
    .await;
    let refs = references(gateway.addr(), root, &root.join("pricing.py"), 1, 5)
        .await
        .unwrap();
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!((refs[0].1, refs[0].2), (5, 12));
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 3);

    asked.store(0, std::sync::atomic::Ordering::SeqCst);
    let refs = references(gateway.addr(), root, &root.join("lib.rs"), 1, 8)
        .await
        .unwrap();
    assert!(refs.is_empty());
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// #442: a reply that is not a list of locations, or an entry without a file or a start, is
/// an error rather than a caller that silently is not there; `null` is the protocol's "none".
#[test]
fn a_malformed_reference_answer_is_an_error_not_a_missing_caller() {
    assert!(
        parse_locations(&serde_json::Value::Null)
            .unwrap()
            .is_empty()
    );
    let good = serde_json::json!([
        { "uri": "file:///w/a.rs", "range": { "start": { "line": 1, "character": 4 } } }
    ]);
    assert_eq!(
        parse_locations(&good).unwrap(),
        [(PathBuf::from("/w/a.rs"), 2, 5)]
    );
    let cases = [
        (serde_json::json!({ "error": "busy" }), "not a list"),
        (
            serde_json::json!([good[0].clone(), { "range": good[0]["range"].clone() }]),
            "reference 2 of 2",
        ),
        (
            serde_json::json!([{ "uri": "file:///w/a.rs" }]),
            "reference 1 of 1",
        ),
        (
            serde_json::json!([{ "uri": "file:///w/a.rs",
                "range": { "start": { "line": -1, "character": 0 } } }]),
            "no file or start position",
        ),
        (
            serde_json::json!([{ "uri": "file:///w/a.rs",
                "range": { "start": { "line": u32::MAX, "character": 0 } } }]),
            "which no file has",
        ),
        (
            serde_json::json!([{ "uri": "file:///w/a.rs",
                "range": { "start": { "line": 0, "character": u32::MAX } } }]),
            "which no file has",
        ),
        (
            serde_json::json!([{ "uri": "file:///w/a.rs",
                "range": { "start": { "line": u64::from(u32::MAX) + 1, "character": 0 } } }]),
            "no file or start position",
        ),
    ];
    for uri in [
        "untitled:Untitled-1",
        "https://example.com/a.rs",
        "file://build-host/w/a.rs",
        "/w/a.rs",
        "a.rs",
    ] {
        let reply = serde_json::json!([{ "uri": uri,
            "range": { "start": { "line": 0, "character": 0 } } }]);
        let err = parse_locations(&reply).unwrap_err().to_string();
        assert!(err.contains("is not a local file URI"), "{uri}: {err}");
    }
    // Percent-encoded and `localhost` spellings are the same local file.
    let spelled = serde_json::json!([
        { "uri": "file:///w/a%20b.rs", "range": { "start": { "line": 0, "character": 0 } } },
        { "uri": "file://localhost/w/a.rs", "range": { "start": { "line": 0, "character": 0 } } }
    ]);
    assert_eq!(
        parse_locations(&spelled).unwrap(),
        [
            (PathBuf::from("/w/a b.rs"), 1, 1),
            (PathBuf::from("/w/a.rs"), 1, 1)
        ]
    );
    for (reply, why) in cases {
        let err = parse_locations(&reply).unwrap_err().to_string();
        assert!(err.contains(why), "{reply}: {err}");
    }
}
