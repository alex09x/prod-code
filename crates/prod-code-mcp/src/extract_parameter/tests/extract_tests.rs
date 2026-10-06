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

use super::super::enclosing::{enclosing_function, with_parameter};
use super::super::hover::type_from_hover;
use super::super::types::ExtractedParameter;

#[test]
fn the_enclosing_function_is_not_the_let_the_expression_sits_in() {
    // What the analyzer really answers: the function, and the local inside it.
    let symbols = serde_json::json!([
        { "name": "move_item", "kind": 12,
          "range": { "start": { "line": 577 }, "end": { "line": 700 } },
          "children": [
            { "name": "removed", "kind": 13,
              "range": { "start": { "line": 628 }, "end": { "line": 628 } } }
          ] }
    ]);
    assert_eq!(
        enclosing_function(&symbols, 629),
        Some(("move_item".to_string(), 578, 701)),
        "a local binding cannot take a parameter, so it is not a candidate"
    );
    assert_eq!(enclosing_function(&symbols, 900), None);
}

#[test]
fn a_hover_that_names_a_binding_gives_its_type_and_anything_else_gives_none() {
    assert_eq!(
        type_from_hover("```rust\nlet decl_end: u32\n```").as_deref(),
        Some("u32")
    );
    assert_eq!(
        type_from_hover("```rust\nlet name: BTreeMap<String, u8>\n```").as_deref(),
        Some("BTreeMap<String, u8>")
    );
    assert_eq!(type_from_hover("```rust\ncore::str\n```"), None);
    assert_eq!(type_from_hover(""), None);
}

#[test]
fn a_parameter_is_added_at_the_end_and_keeps_the_lists_shape() {
    assert_eq!(with_parameter("", "limit: usize"), "limit: usize");
    assert_eq!(
        with_parameter("a: u8, b: u8", "limit: usize"),
        "a: u8, b: u8, limit: usize"
    );
    // One per line, trailing comma: the new one keeps that shape and the indentation.
    assert_eq!(
        with_parameter("\n    a: u8,\n    b: u8,\n", "limit: usize"),
        "\n    a: u8,\n    b: u8,\n    limit: usize,\n"
    );
}

fn report(unmatched: Vec<String>, diagnostics: Vec<String>, applied: bool) -> ExtractedParameter {
    ExtractedParameter {
        symbol: "render".into(),
        root: PathBuf::from("/root"),
        file: "src/lib.rs".into(),
        name: "width_limit".into(),
        ty: "usize".into(),
        parameter: "width_limit: usize".into(),
        expression: "80".into(),
        replaced: 1,
        call_sites: 2,
        rewritten: vec![("/root/src/lib.rs".into(), "pub fn render() {}\n".into())],
        unmatched,
        unreported: Vec::new(),
        diagnostics,
        applied,
    }
}

#[test]
fn the_report_says_what_was_left_out_and_what_the_analyzer_thought() {
    let clean = report(Vec::new(), Vec::new(), false).render(4000);
    assert!(
        clean.contains("new parameter: `width_limit: usize`"),
        "{clean}"
    );
    assert!(clean.contains("2 call site(s) pass it"), "{clean}");
    assert!(
        clean.contains("the analyzer accepts the result: 0 errors"),
        "{clean}"
    );
    assert!(clean.contains("nothing was written"), "{clean}");

    let missed = report(vec!["src/other.rs:9:5".into()], Vec::new(), false).render(4000);
    assert!(
        missed.contains("not given the argument (1 reference"),
        "{missed}"
    );
    assert!(missed.contains("src/other.rs:9:5"), "{missed}");

    // A rejected result explains the usual cause rather than leaving a raw diagnostic.
    let broken = report(
        Vec::new(),
        vec!["cannot find value `n` [E0425] (src/lib.rs:8:17)".into()],
        false,
    )
    .render(4000);
    assert!(
        broken.contains("the analyzer rejects the result"),
        "{broken}"
    );
    assert!(broken.contains("if it names a local"), "{broken}");

    let written = report(Vec::new(), Vec::new(), true).render(4000);
    assert!(written.contains("[applied to 1 file(s)]"), "{written}");
    assert!(!written.contains("nothing was written"), "{written}");
}

#[test]
fn a_diff_longer_than_the_budget_is_cut_and_says_so() {
    let cut = report(Vec::new(), Vec::new(), false).render(10);
    assert!(cut.contains("… diff truncated"), "{cut}");
}
