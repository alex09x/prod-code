/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! How polyglot encapsulate-field treats what the analyzer reports about a Python field:
//! every access it points at is rewritten, and every other kind of answer is a named refusal.

use prod_code_mcp::encapsulate_field::encapsulate_polyglot;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;
use std::net::SocketAddr;

const BOX: &str = "class Box:\n    def __init__(self):\n        self.count: int = 0\n";
const USE: &str = "from box import Box\nb = Box()\nprint(b.count)\nb.count = 3\n";
const USE_TWO: &str = "from box import Box\nb = Box()\nprint(b.count, c.count)\n";

fn workspace(use_text: &str) -> Workspace {
    Workspace::new(&[
        ("box.py", BOX),
        ("use.py", use_text),
        ("notes.txt", "count\n"),
    ])
}

fn symbols() -> Value {
    Value::Array(vec![answers::nested(
        answers::document_symbol("Box", 5, 1, 3, 7),
        vec![answers::document_symbol("count", 8, 3, 3, 14)],
    )])
}

/// A gateway answering the field's symbols with `symbols`, references with `refs`.
async fn gateway(symbols: Value, refs: Value) -> SocketAddr {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => symbols.clone(),
        "textDocument/references" => refs.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
    .addr()
}

fn refs(ws: &Workspace, spots: &[(&str, u32, u32)]) -> Value {
    Value::Array(
        spots
            .iter()
            .flat_map(|(rel, line, col)| {
                answers::locations(&ws.path(rel), &[(*line, *col)])
                    .as_array()
                    .cloned()
                    .unwrap()
            })
            .collect(),
    )
}

async fn run(
    ws: &Workspace,
    remote: SocketAddr,
    apply: bool,
) -> prod_code_mcp::encapsulate_field::EncapsulatedField {
    encapsulate_polyglot(
        remote,
        &ws.root(),
        &ws.path("box.py"),
        None,
        "count",
        None,
        apply,
        false,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn confirmed_accesses_are_rewritten_and_written() {
    let ws = workspace(USE);
    let spots = refs(
        &ws,
        &[("box.py", 3, 14), ("use.py", 3, 9), ("use.py", 4, 3)],
    );
    let done = run(&ws, gateway(symbols(), spots).await, true).await;
    assert_eq!(
        (
            done.owner.as_str(),
            done.ty.as_str(),
            done.reads,
            done.writes,
            done.left_in_file
        ),
        ("Box", "int", 1, 1, 1)
    );
    assert!(
        done.unmatched.is_empty() && done.diagnostics.is_empty(),
        "{:?}",
        done.unmatched
    );
    assert!(done.applied);
    assert_eq!(
        ws.read("use.py"),
        "from box import Box\nb = Box()\nprint(b.get_count())\nb.set_count(3)\n"
    );
    assert_eq!(
        ws.read("box.py"),
        "class Box:\n    def __init__(self):\n        self._count: int = 0\n\n    def get_count(self) -> int:\n        return self._count\n\n    def set_count(self, count: int) -> None:\n        self._count = count\n"
    );
}

#[tokio::test]
async fn unresolved_symbols_leave_every_access_unmatched() {
    let ws = workspace(USE);
    let by_failure = run(
        &ws,
        gateway(answers::failure("analyzer down"), Value::Null).await,
        false,
    )
    .await;
    assert_eq!(by_failure.unmatched.len(), 3, "{:?}", by_failure.unmatched);
    assert!(by_failure.unmatched[0].starts_with("box.py: field symbols could not be resolved: "));
    assert_eq!(
        by_failure.unmatched[1..],
        [
            "use.py:3: possible `count` access could not be matched to `Box`".to_string(),
            "use.py:4: possible `count` access could not be matched to `Box`".to_string(),
        ]
    );
    assert!(!by_failure.applied);

    let silent = run(&ws, gateway(Value::Null, Value::Null).await, false).await;
    assert_eq!(silent.unmatched, by_failure.unmatched[1..]);
    assert_eq!(ws.read("use.py"), USE, "nothing is written");

    let no_refs = run(
        &ws,
        gateway(symbols(), answers::failure("references down")).await,
        false,
    )
    .await;
    assert!(
        no_refs.unmatched[0].starts_with("box.py: analyzer references could not be verified: "),
        "{:?}",
        no_refs.unmatched
    );
}

#[tokio::test]
async fn references_that_do_not_add_up_are_named() {
    let ws = workspace(USE);
    let table: Vec<(Vec<(&str, u32, u32)>, Vec<&str>)> = vec![
        (
            vec![("use.py", 3, 9), ("use.py", 4, 3)],
            vec![
                "box.py: analyzer found 0 field reference(s) in the declaring file, but only 1 could be rewritten",
            ],
        ),
        (
            vec![("box.py", 3, 14), ("use.py", 3, 9)],
            vec![
                "use.py:4: possible `count` access is absent from analyzer references",
                "use.py:4: possible `count` access could not be matched to `Box`",
            ],
        ),
        (
            vec![("box.py", 3, 14), ("use.py", 3, 1), ("use.py", 4, 3)],
            vec!["use.py:3: analyzer reference does not point at `count`"],
        ),
        (
            vec![
                ("box.py", 3, 14),
                ("use.py", 3, 9),
                ("use.py", 4, 3),
                ("use.py", 50, 1),
            ],
            vec!["use.py: analyzer reference points beyond end of file"],
        ),
        (
            vec![
                ("box.py", 3, 14),
                ("use.py", 3, 9),
                ("use.py", 4, 3),
                ("notes.txt", 1, 1),
            ],
            vec!["notes.txt: analyzer reference is outside the supported workspace language"],
        ),
        (
            vec![
                ("box.py", 3, 14),
                ("use.py", 3, 9),
                ("use.py", 4, 3),
                ("gone.py", 1, 1),
            ],
            vec!["gone.py: referenced source could not be read: "],
        ),
    ];
    for (idx, (spots, expected)) in table.into_iter().enumerate() {
        let remote = gateway(symbols(), refs(&ws, &spots)).await;
        let done = run(&ws, remote, true).await;
        assert!(!done.applied, "case {idx} must not write");
        assert_eq!(
            done.unmatched.len(),
            expected.len(),
            "case {idx}: {:?}",
            done.unmatched
        );
        for (got, want) in done.unmatched.iter().zip(&expected) {
            assert!(got.starts_with(want), "case {idx}: {got:?} vs {want:?}");
        }
        assert_eq!(ws.read("use.py"), USE, "case {idx}");
    }
}

#[tokio::test]
async fn a_line_with_more_accesses_than_references_is_not_rewritten() {
    let ws = workspace(USE_TWO);
    let spots = refs(&ws, &[("box.py", 3, 14), ("use.py", 3, 9)]);
    let done = run(&ws, gateway(symbols(), spots).await, true).await;
    assert!(!done.applied);
    assert_eq!(
        done.unmatched,
        vec!["use.py:3: only 2 of 1 analyzer reference(s) could be rewritten".to_string()]
    );
    assert_eq!(ws.read("use.py"), USE_TWO);
}
