/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

fn diag(line: u32, col: u32, code: Option<&str>, message: &str) -> DocDiagnostic {
    DocDiagnostic {
        severity: "error".into(),
        code: code.map(str::to_string),
        message: message.into(),
        line,
        col,
        source: Some("SourceKit".into()),
        note: None,
        end: None,
    }
}

fn error(line: u32, col: u32, message: &str) -> DocDiagnostic {
    diag(line, col, None, message)
}

fn report(file: &str, items: Vec<DocDiagnostic>) -> DiagnosticsReport {
    let errors = items.len();
    DiagnosticsReport {
        file: file.into(),
        errors,
        warnings: 0,
        items,
        preexisting: Vec::new(),
        in_derive: Vec::new(),
        auto_trait: Vec::new(),
        hallucinations: Vec::new(),
    }
}

fn texts(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(f, t)| (f.to_string(), t.to_string()))
        .collect()
}

const BEFORE: &str = "struct Runner {\n    let driver: Driver\n}\n\nenum DesktopKeys {\n    static let save: Key = .save\n    static let page = SettingsPage.general\n}\n\nfunc run() -> Failure? { nil }\n";
const AFTER_SOURCE: &str =
    "struct Runner {\n    let driver: Driver\n}\n\nfunc run() -> Failure? { nil }\n";
const AFTER_TARGET: &str = "enum DesktopKeys {\n    static let save: Key = .save\n    static let page = SettingsPage.general\n}\n";

fn runner_baseline() -> HashMap<String, (DiagnosticsReport, String)> {
    HashMap::from([(
        "Runner.swift".to_string(),
        (
            report(
                "Runner.swift",
                vec![
                    error(2, 17, "cannot find type 'Driver' in scope"),
                    error(6, 22, "cannot find type 'Key' in scope"),
                    error(7, 23, "cannot find 'SettingsPage' in scope"),
                    error(10, 15, "cannot find type 'Failure' in scope"),
                ],
            ),
            BEFORE.to_string(),
        ),
    )])
}

/// #2038: a Swift file outside every SwiftPM target already fails to find its peers. Moving one
/// declaration out leaves some of those errors in place and carries the rest with the moved
/// lines; none of them is the move's, and a name the move breaks still is.
#[test]
fn errors_a_move_leaves_or_carries_are_not_the_moves() {
    let sources = texts(&[
        ("Runner.swift", AFTER_SOURCE),
        ("DesktopKeys.swift", AFTER_TARGET),
    ]);
    let mut reports = vec![
        report(
            "Runner.swift",
            vec![
                error(2, 17, "cannot find type 'Driver' in scope"),
                error(5, 15, "cannot find type 'Failure' in scope"),
            ],
        ),
        report(
            "DesktopKeys.swift",
            vec![
                error(2, 22, "cannot find type 'Key' in scope"),
                error(3, 23, "cannot find 'SettingsPage' in scope"),
                // New: the move broke this name, which resolved before.
                error(3, 5, "cannot find 'Runner' in scope"),
            ],
        ),
    ];
    set_aside_carried(&mut reports, &sources, &runner_baseline());
    assert_eq!(reports[0].errors, 0, "{:?}", reports[0].items);
    assert_eq!(reports[0].preexisting.len(), 2);
    assert_eq!(reports[1].errors, 1, "{:?}", reports[1].items);
    assert_eq!(reports[1].items[0].message, "cannot find 'Runner' in scope");
    assert_eq!(reports[1].preexisting.len(), 2);
}

/// An error moved into another scope of the same file is not a carried line: the per-file
/// comparison refuses it, and nothing here overrides that.
#[test]
fn a_line_moved_within_one_file_keeps_its_error() {
    for (path, before, after) in [
        (
            "src/main.ts",
            "function first() {\n    missing();\n}\n",
            "function first() {\n}\nfunction second() {\n    missing();\n}\n",
        ),
        (
            "src/main.ts",
            "function first() {\n}\nfunction second() {\n    missing();\n}\n",
            "function first() {\n    missing();\n}\n",
        ),
    ] {
        let line_of =
            |text: &str| text.lines().position(|l| l.contains("missing")).unwrap() as u32 + 1;
        let baselines = HashMap::from([(
            path.to_string(),
            (
                report(
                    path,
                    vec![error(line_of(before), 5, "Cannot find name 'missing'")],
                ),
                before.to_string(),
            ),
        )]);
        let mut reports = vec![report(
            path,
            vec![error(line_of(after), 5, "Cannot find name 'missing'")],
        )];
        set_aside_carried(&mut reports, &texts(&[(path, after)]), &baselines);
        assert_eq!(reports[0].errors, 1, "{before:?} -> {after:?}");
    }
}

/// A block cut from a file but not written into another file of the batch is a deletion, not
/// a move: the errors of what is left keep whatever the per-file comparison decided.
#[test]
fn a_cut_that_lands_nowhere_is_not_a_move() {
    let sources = texts(&[("Runner.swift", AFTER_SOURCE)]);
    let mut reports = vec![report(
        "Runner.swift",
        vec![error(5, 15, "cannot find type 'Failure' in scope")],
    )];
    set_aside_carried(&mut reports, &sources, &runner_baseline());
    assert_eq!(reports[0].errors, 1);
}

/// One error before accounts for one after, even when the moved block lands in two files.
#[test]
fn each_carried_error_excuses_one_after() {
    let sources = texts(&[
        ("Runner.swift", AFTER_SOURCE),
        ("DesktopKeys.swift", AFTER_TARGET),
        ("Copy.swift", AFTER_TARGET),
    ]);
    let mut reports = vec![
        report(
            "DesktopKeys.swift",
            vec![error(2, 22, "cannot find type 'Key' in scope")],
        ),
        report(
            "Copy.swift",
            vec![error(2, 22, "cannot find type 'Key' in scope")],
        ),
    ];
    set_aside_carried(&mut reports, &sources, &runner_baseline());
    assert_eq!(reports[0].errors + reports[1].errors, 1);
}

/// The marker of a file the analyzer did not check stays an error: an unchecked file never
/// passes as clean because it was unchecked before the move too (#94, #467).
#[test]
fn an_unchecked_marker_is_never_carried() {
    let code = Some(prod_code_protocol::ANALYZER_PANIC_CODE);
    let mut baselines = runner_baseline();
    let marker = diag(
        1,
        1,
        code,
        "the analyzer panicked; this file was not checked",
    );
    baselines
        .get_mut("Runner.swift")
        .unwrap()
        .0
        .items
        .push(marker.clone());
    let sources = texts(&[
        ("Runner.swift", AFTER_SOURCE),
        ("DesktopKeys.swift", AFTER_TARGET),
    ]);
    let mut reports = vec![report("Runner.swift", vec![marker])];
    set_aside_carried(&mut reports, &sources, &baselines);
    assert_eq!(reports[0].errors, 1);
}

/// Rust keeps its own structural comparison: a moved Rust line can resolve differently.
#[test]
fn rust_files_are_left_to_the_structural_comparison() {
    let before = "fn keep() {}\n\nfn f() { secret() }\n";
    let baselines = HashMap::from([(
        "src/a.rs".to_string(),
        (
            report(
                "src/a.rs",
                vec![error(3, 10, "cannot find function `secret`")],
            ),
            before.to_string(),
        ),
    )]);
    let sources = texts(&[
        ("src/a.rs", "fn keep() {}\n"),
        ("src/b.rs", "fn f() { secret() }\n"),
    ]);
    let mut reports = vec![report(
        "src/b.rs",
        vec![error(1, 10, "cannot find function `secret`")],
    )];
    set_aside_carried(&mut reports, &sources, &baselines);
    assert_eq!(reports[0].errors, 1);
}
