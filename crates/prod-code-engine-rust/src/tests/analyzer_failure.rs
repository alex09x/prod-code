/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #2025: a panic of the analyzer in one part of a diagnostics pass costs that part only.

use crate::RustEngine;

/// An import nobody uses, two adjacent `use` items the analyzer panics on, and a type error.
///
/// The panic is in the `merge_imports` assist, which the unused-import pass never asks for but
/// the analyzer computes with every other assist of a `use` item. It merges the two items into
/// `use crate::parts::{alpha, beta, gen};` by building that text and parsing it, and the
/// parser it uses takes `gen` for the keyword of a later edition: the parsed list has fewer
/// trees than went in, which `SyntaxMappingBuilder::map_children` calls unreachable.
const SOURCE: &str = "\
use std::collections::HashMap;

pub mod parts {
    pub fn alpha() {}
    pub fn beta() {}
    pub fn gen() {}
}

use crate::parts::alpha;
use crate::parts::{beta, gen};

pub fn run() -> u32 {
    alpha();
    beta();
    gen();
    let wrong: u32 = \"text\";
    wrong
}
";

#[test]
fn a_use_item_the_analyzer_panics_on_costs_the_file_only_that_item() {
    let (_temp, path) = super::create_test_fixture();
    std::fs::write(&path, SOURCE).unwrap();
    let engine = RustEngine::load(path.parent().unwrap().parent().unwrap()).unwrap();

    // The pass used to end in the analyzer's panic, and the file had no diagnostics at all.
    let found = engine.snapshot().diagnostics(&path).unwrap();
    let at = |code: &str| -> Vec<u32> {
        found
            .iter()
            .filter(|d| d.code == code)
            .map(|d| d.line)
            .collect()
    };
    assert_eq!(at("E0308"), [16], "the type error is reported: {found:#?}");
    assert_eq!(
        at("unused_imports"),
        [1],
        "an import away from the two items is still checked: {found:#?}"
    );
    // The two items say that they were not checked, as hints on their own lines.
    let notices: Vec<_> = found
        .iter()
        .filter(|d| d.code == crate::snapshot::analyzer_failure::ANALYZER_FAILED)
        .collect();
    assert_eq!(
        notices
            .iter()
            .map(|d| (d.line, d.severity.as_str()))
            .collect::<Vec<_>>(),
        [(9, "weak"), (10, "weak")],
        "{found:#?}"
    );
    assert!(
        notices[0]
            .message
            .contains("not checked for unused imports")
            && notices[0].message.contains("mapping more input nodes"),
        "{}",
        notices[0].message
    );

    // An editor is sent the same: the hints, the warning and the error.
    let published = engine.editor_diagnostics(&path).unwrap();
    let published = published.as_array().unwrap();
    let severities = |code: &str| -> Vec<u64> {
        published
            .iter()
            .filter(|d| d["code"] == code)
            .map(|d| d["severity"].as_u64().unwrap())
            .collect()
    };
    assert_eq!(severities("analyzer-failed"), [4, 4], "{published:#?}");
    assert_eq!(severities("E0308"), [1], "{published:#?}");
    assert_eq!(severities("unused_imports"), [2], "{published:#?}");
}
