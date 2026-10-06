/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeSet, HashMap};

use crate::diagnostics::annotate::{STALE_REFERENCE, annotate_missing_symbols};
use crate::diagnostics::types::{DiagnosticsReport, DocDiagnostic};

#[test]
fn errors_on_lines_using_a_removed_symbol_get_a_note() {
    let mut reports = vec![DiagnosticsReport {
        file: "crates/gateway/src/main.rs".to_string(),
        errors: 1,
        warnings: 0,
        items: vec![
            DocDiagnostic {
                severity: "error".to_string(),
                code: Some("E0282".to_string()),
                message: "type annotations needed".to_string(),
                line: 2,
                col: 16,
                source: None,
                note: None,
                end: None,
            },
            DocDiagnostic {
                severity: "hint".to_string(),
                code: None,
                message: "unused".to_string(),
                line: 3,
                col: 1,
                source: None,
                note: None,
                end: None,
            },
        ],
        preexisting: vec![],
        in_derive: vec![],
        auto_trait: vec![],
        hallucinations: vec![],
    }];
    let mut sources = HashMap::new();
    sources.insert(
        "crates/gateway/src/main.rs".to_string(),
        "fn run() {\n    if let Some(s) = workspace::shared_target_dir(&ws) {}\n    let unused = 1;\n}\n".to_string(),
    );
    let missing = vec![(
        "shared_target_dir".to_string(),
        "crates/gateway/src/workspace.rs".to_string(),
    )];
    annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
    let note = reports[0].items[0].note.as_deref().unwrap();
    assert!(
        note.contains("`shared_target_dir`") && note.contains("crates/gateway/src/workspace.rs"),
        "{note}"
    );
    assert!(reports[0].items[1].note.is_none(), "hints are left alone");
    assert_eq!(
        reports[0].items.len(),
        2,
        "a line that already has an error gets no extra warning"
    );
    assert!(
        reports[0]
            .render()
            .contains("note: this line uses `shared_target_dir`")
    );
}

#[test]
fn silent_lines_using_a_removed_symbol_get_a_synthesised_warning() {
    let mut reports = vec![
        DiagnosticsReport {
            file: "crates/gateway/src/main.rs".to_string(),
            errors: 0,
            warnings: 0,
            items: vec![],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![],
        },
        DiagnosticsReport {
            file: "crates/gateway/src/workspace.rs".to_string(),
            errors: 0,
            warnings: 0,
            items: vec![],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![],
        },
    ];
    let mut sources = HashMap::new();
    sources.insert(
        "crates/gateway/src/main.rs".to_string(),
        "fn run() {\n    workspace::touch_last_used(&server_workspace);\n}\n".to_string(),
    );
    sources.insert(
        "crates/gateway/src/workspace.rs".to_string(),
        "/// touch_last_used used to live here\npub fn record_last_used() {}\n".to_string(),
    );
    let missing = vec![(
        "touch_last_used".to_string(),
        "crates/gateway/src/workspace.rs".to_string(),
    )];
    annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
    assert_eq!(reports[0].warnings, 1);
    assert_eq!(reports[0].items.len(), 1);
    let item = &reports[0].items[0];
    assert_eq!((item.line, item.col), (2, 16));
    assert_eq!(item.code.as_deref(), Some(STALE_REFERENCE));
    assert!(
        item.note
            .as_deref()
            .unwrap()
            .contains("crates/gateway/src/workspace.rs")
    );
    assert!(
        reports[1].items.is_empty(),
        "the file the symbol vanished from is not flagged"
    );
}

#[test]
fn doc_and_line_comments_mentioning_removed_symbol_are_not_flagged() {
    let mut reports = vec![DiagnosticsReport {
        file: "crates/gateway/src/workspace.rs".to_string(),
        errors: 0,
        warnings: 0,
        items: vec![],
        preexisting: vec![],
        in_derive: vec![],
        auto_trait: vec![],
        hallucinations: vec![],
    }];
    let mut sources = HashMap::new();
    sources.insert(
        "crates/gateway/src/workspace.rs".to_string(),
        "/// covers `excess` bytes. Only engines without a session\n\
//! covers the whole tree\n\
// ordinary line comment with covers\n\
pub fn work() {}\n"
            .to_string(),
    );
    let missing = vec![("covers".to_string(), "crates/engine/src/lib.rs".to_string())];
    annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
    assert_eq!(reports[0].warnings, 0, "no false warnings on comments");
    assert!(reports[0].items.is_empty(), "{:?}", reports[0].items);

    let mut reports_with_warning = vec![DiagnosticsReport {
        file: "crates/gateway/src/workspace.rs".to_string(),
        errors: 0,
        warnings: 1,
        items: vec![DocDiagnostic {
            severity: "warning".to_string(),
            code: Some("dead_code".to_string()),
            message: "unused".to_string(),
            line: 1,
            col: 1,
            source: None,
            note: None,
            end: None,
        }],
        preexisting: vec![],
        in_derive: vec![],
        auto_trait: vec![],
        hallucinations: vec![],
    }];
    annotate_missing_symbols(
        &mut reports_with_warning,
        &sources,
        &missing,
        &BTreeSet::new(),
    );
    assert!(reports_with_warning[0].items[0].note.is_none());
}

#[test]
fn block_comments_nested_and_multiline_are_not_flagged() {
    let mut reports = vec![DiagnosticsReport {
        file: "crates/gateway/src/workspace.rs".to_string(),
        errors: 0,
        warnings: 0,
        items: vec![],
        preexisting: vec![],
        in_derive: vec![],
        auto_trait: vec![],
        hallucinations: vec![],
    }];
    let mut sources = HashMap::new();
    sources.insert(
        "crates/gateway/src/workspace.rs".to_string(),
        "/*\n * multiline comment\n * covers\n */\n\
/* outer /* inner covers */ still comment */\n\
/** doc block comment covers */\n\
/*! inner doc block covers */\n\
pub fn work() {}\n"
            .to_string(),
    );
    let missing = vec![("covers".to_string(), "crates/engine/src/lib.rs".to_string())];
    annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
    assert_eq!(reports[0].warnings, 0);
    assert!(reports[0].items.is_empty());
}

#[test]
fn strings_and_literals_are_not_flagged_as_stale_references() {
    let mut reports = vec![DiagnosticsReport {
        file: "crates/gateway/src/workspace.rs".to_string(),
        errors: 0,
        warnings: 0,
        items: vec![],
        preexisting: vec![],
        in_derive: vec![],
        auto_trait: vec![],
        hallucinations: vec![],
    }];
    let mut sources = HashMap::new();
    sources.insert(
        "crates/gateway/src/workspace.rs".to_string(),
        "fn run() {\n\
    let a = \"covers\";\n\
    let b = \"multiline \\\n             covers string\";\n\
    let c = \"escaped \\\" covers \\\"\";\n\
    let d = r#\"raw covers string\"#;\n\
    let e = r##\"nested # raw covers string\"##;\n\
    let f = b\"byte covers\";\n\
    let g = br#\"raw byte covers\"#;\n\
    let h = c\"c string covers\";\n\
    let i = 'c';\n\
    let j = '\\'';\n\
    let k = b'\\n';\n\
}\n\
fn lifetime<'covers>(x: &'covers str) -> &'covers str { x }\n"
            .to_string(),
    );
    let missing = vec![("covers".to_string(), "crates/engine/src/lib.rs".to_string())];
    annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
    assert_eq!(reports[0].warnings, 0, "{:?}", reports[0].items);
    assert!(reports[0].items.is_empty());
}

#[test]
fn real_code_tokens_attach_at_token_position_even_with_preceding_prose() {
    let mut reports = vec![DiagnosticsReport {
        file: "crates/gateway/src/workspace.rs".to_string(),
        errors: 0,
        warnings: 0,
        items: vec![],
        preexisting: vec![],
        in_derive: vec![],
        auto_trait: vec![],
        hallucinations: vec![],
    }];
    let mut sources = HashMap::new();
    sources.insert(
        "crates/gateway/src/workspace.rs".to_string(),
        r#"fn run() {
    /* covers comment */ workspace::covers(&ws);
    let msg = "covers in string"; covers();
    r#covers();
    self.covers();
    // 🦀 covers emoji
    let 🦀 = covers();
}
"#
        .to_string(),
    );
    let missing = vec![("covers".to_string(), "crates/engine/src/lib.rs".to_string())];
    annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
    assert_eq!(reports[0].warnings, 5, "{:?}", reports[0].items);

    let d0 = &reports[0].items[0];
    assert_eq!(d0.line, 2);
    assert_eq!(d0.col, 37);

    let d1 = &reports[0].items[1];
    assert_eq!(d1.line, 3);
    assert_eq!(d1.col, 35);

    let d2 = &reports[0].items[2];
    assert_eq!(d2.line, 4);
    assert_eq!(d2.col, 5);

    let d3 = &reports[0].items[3];
    assert_eq!(d3.line, 5);
    assert_eq!(d3.col, 10);

    let d4 = &reports[0].items[4];
    assert_eq!(d4.line, 7);
    assert_eq!(d4.col, 14);
}

#[test]
fn whole_identifier_boundaries_do_not_flag_substring_matches() {
    let mut reports = vec![DiagnosticsReport {
        file: "crates/gateway/src/workspace.rs".to_string(),
        errors: 0,
        warnings: 0,
        items: vec![],
        preexisting: vec![],
        in_derive: vec![],
        auto_trait: vec![],
        hallucinations: vec![],
    }];
    let mut sources = HashMap::new();
    sources.insert(
        "crates/gateway/src/workspace.rs".to_string(),
        "fn run() {\n\
    undercovers();\n\
    covers_everything();\n\
    my_covers_fn();\n\
}\n"
        .to_string(),
    );
    let missing = vec![("covers".to_string(), "crates/engine/src/lib.rs".to_string())];
    annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
    assert_eq!(reports[0].warnings, 0);
    assert!(reports[0].items.is_empty());
}

#[test]
fn non_rust_files_preserve_existing_behavior_and_utf16_columns() {
    let mut reports = vec![DiagnosticsReport {
        file: "gateway/workspace.go".to_string(),
        errors: 0,
        warnings: 0,
        items: vec![],
        preexisting: vec![],
        in_derive: vec![],
        auto_trait: vec![],
        hallucinations: vec![],
    }];
    let mut sources = HashMap::new();
    sources.insert(
        "gateway/workspace.go".to_string(),
        "package main\nfunc run() {\n    // 🚀 covers()\n}\n".to_string(),
    );
    let missing = vec![("covers".to_string(), "engine/lib.go".to_string())];
    annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
    assert_eq!(reports[0].warnings, 1);
    assert_eq!(reports[0].items[0].line, 3);
    assert_eq!(reports[0].items[0].col, 11);
}
