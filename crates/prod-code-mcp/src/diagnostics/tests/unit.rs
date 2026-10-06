/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::diagnostics::filter::{
    UNLINKED_FILE, is_auto_trait_bound, mentions_identifier, refuse_unchecked,
    set_aside_derive_expansions, set_aside_preexisting, symbol_names,
};
use crate::diagnostics::parse::{classify_hallucination, parse_items};
use crate::diagnostics::types::{DiagnosticsReport, DocDiagnostic};

#[test]
fn inactive_code_hints_are_dropped_and_counts_ignore_them() {
    let result = serde_json::json!({ "items": [
        { "range": { "start": { "line": 3, "character": 4 }, "end": { "line": 3, "character": 5 } }, "severity": 4,
          "code": "inactive-code", "message": "code is inactive due to #[cfg] directives: unix is enabled" },
        { "range": { "start": { "line": 10, "character": 8 }, "end": { "line": 10, "character": 9 } }, "severity": 1,
          "code": "E0425", "message": "cannot find value `x` in this scope" },
        { "range": { "start": { "line": 12, "character": 1 }, "end": { "line": 12, "character": 2 } }, "severity": 4,
          "code": "unused_variables", "message": "unused variable" }
    ]});
    let report = parse_items("src/lib.rs", &result);
    assert_eq!(report.errors, 1);
    assert_eq!(report.warnings, 0);
    assert_eq!(report.items.len(), 2, "{:?}", report.items);
    assert!(
        report
            .items
            .iter()
            .all(|d| d.code.as_deref() != Some("inactive-code"))
    );
    assert_eq!(report.items[0].line, 11);
    assert_eq!(report.items[0].col, 9);
}

#[test]
fn symbol_names_walks_flat_and_hierarchical_results() {
    let flat = serde_json::json!([
        { "name": "shared_target_dir", "kind": 12 },
        { "name": "Tracked", "kind": 23 }
    ]);
    assert_eq!(
        symbol_names(&flat).into_iter().collect::<Vec<_>>(),
        vec!["Tracked".to_string(), "shared_target_dir".to_string()]
    );
    let tree = serde_json::json!([
        { "name": "Outer", "kind": 23, "children": [ { "name": "inner", "kind": 6 } ] }
    ]);
    assert!(symbol_names(&tree).contains("inner"));
    // A function's `let` bindings are listed as variables and are nobody else's to use.
    let with_locals = serde_json::json!([
        { "name": "run_safe_delete", "kind": 12, "children": [
            { "name": "edit", "kind": 13 }, { "name": "touched", "kind": 13 }
        ] },
        { "name": "LIMIT", "kind": 14 }
    ]);
    assert_eq!(
        symbol_names(&with_locals).into_iter().collect::<Vec<_>>(),
        vec!["LIMIT".to_string(), "run_safe_delete".to_string()]
    );
}

#[test]
fn mentions_identifier_matches_whole_words_only() {
    assert!(mentions_identifier(
        "    let d = workspace::shared_target_dir(&ws);",
        "shared_target_dir"
    ));
    assert!(!mentions_identifier(
        "    let d = shared_target_dir_renamed(&ws);",
        "shared_target_dir"
    ));
    assert!(!mentions_identifier(
        "    let x = my_shared_target_dir;",
        "shared_target_dir"
    ));
}

fn diagnostic(severity: &str, message: &str, line: u32) -> DocDiagnostic {
    DocDiagnostic {
        severity: severity.to_string(),
        code: Some("E0282".to_string()),
        message: message.to_string(),
        line,
        col: 3,
        source: None,
        note: None,
        end: None,
    }
}

fn report_of(items: Vec<DocDiagnostic>) -> DiagnosticsReport {
    DiagnosticsReport {
        file: "src/messages.rs".to_string(),
        errors: items.iter().filter(|d| d.severity == "error").count(),
        warnings: items.iter().filter(|d| d.severity == "warning").count(),
        items,
        preexisting: vec![],
        in_derive: vec![],
        auto_trait: vec![],
        hallucinations: vec![],
    }
}

#[test]
fn an_error_the_file_already_had_is_not_the_edits() {
    // Two derives the analyzer cannot type on disk; the edit moves them down a line and
    // adds a third error of the same kind, and one of its own.
    let before_text = "#[derive(Deserialize)]\nstruct A;\n#[derive(Deserialize)]\nstruct B;\n";
    let before = report_of(vec![
        diagnostic("error", "type annotations needed", 1),
        diagnostic("error", "type annotations needed", 3),
    ]);
    let text = "use x;\n#[derive(Deserialize)]\nstruct A;\n#[derive(Deserialize)]\nstruct B;\n\
                #[derive(Deserialize)]\nstruct C(u8);\nfn f() -> u8 { \"\" }\n";
    let mut report = report_of(vec![
        diagnostic("error", "type annotations needed", 2),
        diagnostic("error", "type annotations needed", 4),
        diagnostic("error", "type annotations needed", 6),
        diagnostic("error", "expected u8, found &str", 8),
        diagnostic("warning", "type annotations needed", 2),
    ]);
    set_aside_preexisting(&mut report, text, &before, before_text);
    assert_eq!(report.preexisting.len(), 2, "{:?}", report.preexisting);
    let lines: Vec<u32> = report.items.iter().map(|d| d.line).collect();
    assert_eq!(
        lines,
        vec![6, 8, 2],
        "a third copy and a new severity are the edit's"
    );
    assert_eq!((report.errors, report.warnings), (2, 1));
    let shown = report.render();
    assert!(
        shown.contains("src/messages.rs: 2 error(s), 1 warning(s)"),
        "{shown}"
    );
    assert!(
        shown.contains("2 diagnostic(s) the file already had before this edit are not counted: 2× type annotations needed [E0282]"),
        "{shown}"
    );
}

#[test]
fn a_preexisting_error_is_removed_from_stream_interceptions() {
    let text = "fn call() { value.missing(); }\n";
    let error = DocDiagnostic {
        code: Some("unresolved-method".to_string()),
        ..diagnostic("error", "no method named `missing` found", 1)
    };
    let before = report_of(vec![error.clone()]);
    let mut report = report_of(vec![error]);
    report.hallucinations = report
        .items
        .iter()
        .filter_map(classify_hallucination)
        .collect();
    assert_eq!(report.hallucinations.len(), 1);

    set_aside_preexisting(&mut report, text, &before, text);

    assert!(report.items.is_empty());
    assert_eq!(report.errors, 0);
    assert!(report.hallucinations.is_empty());
}

#[test]
fn inference_failing_in_a_derive_expansion_is_not_counted_even_in_a_new_file() {
    let text = "use serde::Deserialize;\n\n#[derive(Debug, Deserialize)]\npub struct P {\n    pub a: u32,\n}\nfn f() -> u8 { \"\" }\n";
    let mut report = report_of(vec![
        DocDiagnostic {
            code: Some("E0282".into()),
            ..diagnostic("error", "type annotations needed", 3)
        },
        // Anything else on a derive line, or E0282 anywhere else, still counts.
        DocDiagnostic {
            code: Some("E0277".into()),
            ..diagnostic("error", "the trait bound is not satisfied", 3)
        },
        DocDiagnostic {
            code: Some("E0282".into()),
            ..diagnostic("error", "type annotations needed", 7)
        },
    ]);
    set_aside_derive_expansions(&mut report, text);
    assert_eq!(report.in_derive.len(), 1);
    let lines: Vec<(u32, Option<&str>)> = report
        .items
        .iter()
        .map(|d| (d.line, d.code.as_deref()))
        .collect();
    assert_eq!(lines, [(3, Some("E0277")), (7, Some("E0282"))]);
    assert_eq!(report.errors, 2);
    assert!(
        report
            .render()
            .contains("1 \"type annotations needed\" on a #[derive(...)] line are not counted"),
        "{}",
        report.render()
    );
}

/// An unproven `Send` bound in a Rust file is shown and not counted (#327): rust-analyzer
/// does not prove it through a recursive `async fn`, where rustc does. Other E0277, and the
/// same message in a file of another language, still count.
#[test]
fn an_unproven_auto_trait_bound_is_shown_and_not_counted() {
    let text = "fn a() {}\nfn b() {}\nfn c() {}\n";
    let mut report = report_of(vec![
        DocDiagnostic {
            code: Some("E0277".into()),
            ..diagnostic(
                "error",
                "the trait bound `NonNull<()>: Send` is not satisfied",
                1,
            )
        },
        DocDiagnostic {
            code: Some("E0277".into()),
            ..diagnostic(
                "error",
                "`Rc<u8>` cannot be shared between threads safely",
                2,
            )
        },
        DocDiagnostic {
            code: Some("E0277".into()),
            ..diagnostic("error", "the trait bound `u8: Display` is not satisfied", 3)
        },
    ]);
    report.file = "src/main.rs".into();
    set_aside_derive_expansions(&mut report, text);
    assert_eq!(report.auto_trait.len(), 2);
    assert_eq!(report.errors, 1);
    let rendered = report.render();
    assert!(
        rendered.contains("2 unproven Send/Sync/Unpin bound(s) are not counted")
            && rendered.contains(
                "unconfirmed: the trait bound `NonNull<()>: Send` is not satisfied (src/main.rs:1:"
            ),
        "{rendered}"
    );
    assert!(is_auto_trait_bound(
        "`Rc<u8>` cannot be sent between threads safely"
    ));
    let mut other = report_of(vec![DocDiagnostic {
        code: Some("E0277".into()),
        ..diagnostic(
            "error",
            "the trait bound `NonNull<()>: Send` is not satisfied",
            1,
        )
    }]);
    other.file = "main.swift".into();
    set_aside_derive_expansions(&mut other, text);
    assert_eq!(other.errors, 1, "only a Rust file's");
}

#[test]
fn a_file_the_analyzer_could_not_check_is_never_set_aside() {
    let text = "fn a() {}\n";
    let panic = DocDiagnostic {
        code: Some(prod_code_protocol::ANALYZER_PANIC_CODE.to_string()),
        ..diagnostic(
            "error",
            "rust-analyzer panicked while checking this file",
            1,
        )
    };
    let before = report_of(vec![panic.clone()]);
    let mut report = report_of(vec![panic]);
    set_aside_preexisting(&mut report, text, &before, text);
    assert!(report.preexisting.is_empty());
    assert_eq!(
        report.errors, 1,
        "still an error: nothing in the file was checked"
    );
}

fn unlinked(severity: &str) -> DocDiagnostic {
    DocDiagnostic {
        code: Some(UNLINKED_FILE.to_string()),
        ..diagnostic(
            severity,
            "This file is not included in any crates, so rust-analyzer can't offer IDE services.",
            1,
        )
    }
}

/// The analyzer's `unlinked-file` hint in a proposal is an error with a note, and the message
/// stays the analyzer's: nothing claims the file has a type error (#467).
#[test]
fn an_unlinked_proposal_is_counted_and_explained() {
    let other = DocDiagnostic {
        code: Some("unused_variables".into()),
        ..diagnostic("hint", "unused variable", 3)
    };
    let mut report = report_of(vec![unlinked("hint"), other]);
    report.file = "tests/new.rs".into();
    assert!(report.ok(), "the analyzer's answer alone counts nothing");
    refuse_unchecked(&mut report);
    assert!(!report.ok());
    assert_eq!((report.errors, report.warnings), (1, 0));
    let item = &report.items[0];
    assert_eq!(item.severity, "error");
    assert!(
        item.message
            .starts_with("This file is not included in any crates")
    );
    let note = item.note.as_deref().unwrap();
    assert!(
        note.contains("not type-checked")
            && note.contains("not because an error was found")
            && note.contains("--all-targets"),
        "{note}"
    );
    assert_eq!(
        report.items[1].severity, "hint",
        "other hints are left alone"
    );
    assert!(report.items[1].note.is_none());
    let rendered = report.render();
    assert!(
        rendered.contains("tests/new.rs: 1 error(s), 0 warning(s)")
            && rendered.contains("error: This file is not included in any crates")
            && rendered.contains("[unlinked-file] (tests/new.rs:1:3)")
            && rendered.contains("note: rust-analyzer includes this file in no crate"),
        "{rendered}"
    );
}

/// Control: a report without `unlinked-file` keeps its items and counts.
#[test]
fn a_linked_report_is_left_as_it_is() {
    let mut report = report_of(vec![
        diagnostic("warning", "type annotations needed", 2),
        DocDiagnostic {
            code: Some("unused_variables".into()),
            ..diagnostic("hint", "unused variable", 3)
        },
    ]);
    refuse_unchecked(&mut report);
    assert!(report.ok());
    assert_eq!((report.errors, report.warnings), (0, 1));
    assert!(report.items.iter().all(|d| d.note.is_none()));
}

/// A file already unlinked on disk was unchecked before the edit too; that does not set the
/// proposal's `unlinked-file` aside.
#[test]
fn an_unlinked_file_is_never_set_aside() {
    let text = "pub fn helper() -> u32 {\n    1\n}\n";
    let before = report_of(vec![unlinked("hint")]);
    let mut report = report_of(vec![unlinked("hint")]);
    set_aside_preexisting(&mut report, text, &before, text);
    assert!(report.preexisting.is_empty(), "{:?}", report.preexisting);
    refuse_unchecked(&mut report);
    assert_eq!(report.errors, 1);
}
