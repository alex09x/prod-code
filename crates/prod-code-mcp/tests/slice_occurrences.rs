/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::slice::{self, SliceOptions};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

async fn distinct_methods(dataflow: bool, same_line: bool, same_target: bool) {
    let source = if same_line {
        "fn seed(left: Left, right: Right) -> i32 {\n    /* 😀 */ left.run() + right.run()\n}\n"
    } else {
        "fn seed(left: Left, right: Right) -> i32 {\n    let first = left.run();\n    let second = right.run();\n    first + second\n}\n"
    };
    let ws = Workspace::new(&[
        ("src/lib.rs", source),
        ("src/left.rs", "fn run() -> i32 {\n    1\n}\n"),
        ("src/right.rs", "fn run() -> i32 {\n    2\n}\n"),
    ]);
    let root = ws.root();
    let positions: Vec<(u64, u64)> = source
        .lines()
        .enumerate()
        .flat_map(|(line, text)| {
            text.match_indices(".run")
                .map(move |(byte, _)| (line as u64, text[..byte + 1].encode_utf16().count() as u64))
        })
        .collect();
    assert_eq!(positions.len(), 2);
    let expected = positions.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let left = url::Url::from_file_path(root.join("src/left.rs")).unwrap();
    let right = url::Url::from_file_path(root.join("src/right.rs")).unwrap();
    let gateway = ScriptedGateway::start(move |method, params| {
        let uri = params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .unwrap_or("");
        match method {
            "textDocument/documentSymbol" => {
                if uri.ends_with("lib.rs") {
                    json!([answers::document_symbol(
                        "seed",
                        12,
                        1,
                        source.lines().count() as u32,
                        4
                    )])
                } else {
                    json!([answers::document_symbol("run", 12, 1, 3, 4)])
                }
            }
            "textDocument/definition" if uri.ends_with("lib.rs") => {
                let at = (
                    params
                        .pointer("/position/line")
                        .and_then(Value::as_u64)
                        .unwrap(),
                    params
                        .pointer("/position/character")
                        .and_then(Value::as_u64)
                        .unwrap(),
                );
                if let Some(index) = positions.iter().position(|p| *p == at) {
                    recorded.lock().unwrap().push(at);
                    let target = if index == 0 || same_target {
                        &left
                    } else {
                        &right
                    };
                    json!([{"uri": target.as_str(), "range": {
                        "start": {"line": 0, "character": 3},
                        "end": {"line": 0, "character": 6}
                    }}])
                } else {
                    Value::Null
                }
            }
            _ => Value::Null,
        }
    })
    .await;
    let report = slice::slice_with_options(
        gateway.addr(),
        &root,
        &root.join("src/lib.rs"),
        1,
        4,
        SliceOptions {
            depth: 2,
            dataflow,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        expected,
        "each source occurrence must be queried"
    );
    let mut files: Vec<_> = report.items.iter().map(|item| item.file.as_str()).collect();
    files.sort();
    let expected_files = if same_target {
        vec!["src/left.rs", "src/lib.rs"]
    } else {
        vec!["src/left.rs", "src/lib.rs", "src/right.rs"]
    };
    assert_eq!(files, expected_files, "{}", report.render());
    assert!(report.is_complete(), "{}", report.render());
}

#[tokio::test]
async fn distinct_receivers_on_one_line_keep_utf16_positions() {
    distinct_methods(false, true, false).await;
}

#[tokio::test]
async fn dataflow_resolves_both_retained_method_occurrences() {
    distinct_methods(true, false, false).await;
}

#[tokio::test]
async fn repeated_targets_are_deduplicated_after_both_occurrences_resolve() {
    distinct_methods(false, true, true).await;
}

#[tokio::test]
async fn repeated_occurrences_exceeding_the_query_budget_report_a_gap() {
    let source = format!("fn seed() {{\n{}}}\n", "    helper();\n".repeat(70));
    let ws = Workspace::new(&[("src/lib.rs", &source)]);
    let root = ws.root();
    let count = source.lines().count() as u32;
    let queries = Arc::new(Mutex::new(0));
    let recorded = queries.clone();
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => json!([answers::document_symbol("seed", 12, 1, count, 4)]),
        "textDocument/definition" => {
            *recorded.lock().unwrap() += 1;
            Value::Null
        }
        _ => Value::Null,
    })
    .await;
    let report = slice::slice(
        gateway.addr(),
        &root,
        &root.join("src/lib.rs"),
        1,
        4,
        2,
        slice::DEFAULT_MAX_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(*queries.lock().unwrap(), 64, "query work remains bounded");
    assert!(!report.has_complete_evidence());
    assert!(!report.is_complete());
    assert!(
        report.render().contains("occurrence"),
        "{}",
        report.render()
    );
}
