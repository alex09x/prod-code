/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Errors a batch carries unchanged from one edited file into another (#2038).
//!
//! A move cuts lines out of one file and writes them, verbatim, into another. When the analyzer
//! already reported an error on such a line before the move, it reports the same error on the
//! same text after it; the error is not the move's. The per-file comparison cannot see this: the
//! new file has no "before", and outside Rust a cut leaves no line correspondence it trusts. A
//! Swift script compiled through an explicit `swiftc` file list, outside every SwiftPM target, is
//! checked by the analyzer alone, so each peer declaration it uses is already "cannot find
//! 'Driver' in scope" on disk, and every one of those lines was reported as the move's own error.

use std::collections::HashMap;

use crate::diagnostics::filter::{identity, refresh_hallucinations};
use crate::diagnostics::types::{DiagnosticsReport, DocDiagnostic};

type Key = (
    (String, Option<String>, String, String),
    u32,
    Option<String>,
);

/// The diagnostic's content, the trimmed text of its line and its column from the first
/// non-blank character, so a line keeps its key when it moves to another indentation.
fn key(d: &DocDiagnostic, text: &str) -> Key {
    let line = text
        .lines()
        .nth(d.line.saturating_sub(1) as usize)
        .unwrap_or("");
    let indent = line.chars().take_while(|c| c.is_whitespace()).count() as u32;
    (
        identity(d, text),
        d.col.saturating_sub(indent),
        d.source.clone(),
    )
}

/// Moves to `preexisting` each remaining error of a non-Rust edited file that some edited file
/// already had before the batch, on a line of the same text at the same column with the same
/// message and code. Every error before the batch accounts for at most one after it, and those
/// already set aside by the per-file comparison are spent first. Rust keeps its structural
/// comparison alone (#1286): a moved Rust line can resolve differently in its new module.
///
/// `baselines` holds, per displayed file, the diagnostics and text on disk; `sources` the
/// proposed text of each file.
pub fn set_aside_carried(
    reports: &mut [DiagnosticsReport],
    sources: &HashMap<String, String>,
    baselines: &HashMap<String, (DiagnosticsReport, String)>,
) {
    let mut before: HashMap<Key, usize> = HashMap::new();
    for (report, text) in baselines.values() {
        for d in report.items.iter().filter(|d| d.severity == "error") {
            *before.entry(key(d, text)).or_default() += 1;
        }
    }
    for report in reports.iter() {
        let Some(text) = sources.get(&report.file) else {
            continue;
        };
        for d in &report.preexisting {
            if let Some(n) = before.get_mut(&key(d, text)) {
                *n = n.saturating_sub(1);
            }
        }
    }
    for report in reports.iter_mut() {
        if report.file.ends_with(".rs") {
            continue;
        }
        let Some(text) = sources.get(&report.file) else {
            continue;
        };
        let items = std::mem::take(&mut report.items);
        let mut moved = false;
        for d in items {
            let count = (d.severity == "error")
                .then(|| before.get_mut(&key(&d, text)))
                .flatten();
            match count {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    moved = true;
                    report.preexisting.push(d);
                }
                _ => report.items.push(d),
            }
        }
        if moved {
            report.errors = report
                .items
                .iter()
                .filter(|d| d.severity == "error")
                .count();
            report.warnings = report
                .items
                .iter()
                .filter(|d| d.severity == "warning")
                .count();
            refresh_hallucinations(report);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(line: u32, col: u32, message: &str) -> DocDiagnostic {
        DocDiagnostic {
            severity: "error".into(),
            code: None,
            message: message.into(),
            line,
            col,
            source: Some("SourceKit".into()),
            note: None,
            end: None,
        }
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

    const BEFORE: &str = "struct Runner {\n    let driver: Driver\n}\n\nenum DesktopKeys {\n    static let save: Key = .save\n    static let page = SettingsPage.general\n}\n";
    const AFTER_SOURCE: &str = "struct Runner {\n    let driver: Driver\n}\n";
    const AFTER_TARGET: &str = "enum DesktopKeys {\n    static let save: Key = .save\n    static let page = SettingsPage.general\n}\n";

    /// #2038: a Swift file outside every SwiftPM target already fails to find its peers; moving
    /// one declaration out carries those errors with the moved lines. None of them is the move's.
    #[test]
    fn errors_carried_with_the_moved_lines_are_not_the_moves() {
        let baselines = HashMap::from([(
            "Runner.swift".to_string(),
            (
                report(
                    "Runner.swift",
                    vec![
                        error(2, 17, "cannot find type 'Driver' in scope"),
                        error(6, 22, "cannot find type 'Key' in scope"),
                        error(7, 23, "cannot find 'SettingsPage' in scope"),
                    ],
                ),
                BEFORE.to_string(),
            ),
        )]);
        let sources = HashMap::from([
            ("Runner.swift".to_string(), AFTER_SOURCE.to_string()),
            ("DesktopKeys.swift".to_string(), AFTER_TARGET.to_string()),
        ]);
        let mut reports = vec![
            report(
                "Runner.swift",
                vec![error(2, 17, "cannot find type 'Driver' in scope")],
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
        set_aside_carried(&mut reports, &sources, &baselines);
        assert_eq!(reports[0].errors, 0, "{:?}", reports[0].items);
        assert_eq!(reports[0].preexisting.len(), 1);
        assert_eq!(reports[1].errors, 1, "{:?}", reports[1].items);
        assert_eq!(reports[1].items[0].message, "cannot find 'Runner' in scope");
        assert_eq!(reports[1].preexisting.len(), 2);
    }

    /// One error before accounts for one after, even when the moved line is now in two files,
    /// and an error the per-file comparison already set aside is not counted twice.
    #[test]
    fn each_error_before_excuses_one_after() {
        let text = "let a = Missing()\n";
        let baselines = HashMap::from([(
            "A.swift".to_string(),
            (
                report(
                    "A.swift",
                    vec![error(1, 9, "cannot find 'Missing' in scope")],
                ),
                text.to_string(),
            ),
        )]);
        let sources = HashMap::from([
            ("A.swift".to_string(), text.to_string()),
            ("B.swift".to_string(), text.to_string()),
        ]);
        let mut a = report("A.swift", Vec::new());
        a.preexisting
            .push(error(1, 9, "cannot find 'Missing' in scope"));
        let mut reports = vec![
            a,
            report(
                "B.swift",
                vec![error(1, 9, "cannot find 'Missing' in scope")],
            ),
        ];
        set_aside_carried(&mut reports, &sources, &baselines);
        assert_eq!(
            reports[1].errors, 1,
            "the one error before is already spent"
        );
    }

    /// Rust keeps its own structural comparison: a moved Rust line can resolve differently.
    #[test]
    fn rust_files_are_left_to_the_structural_comparison() {
        let text = "fn f() { secret() }\n";
        let baselines = HashMap::from([(
            "src/a.rs".to_string(),
            (
                report(
                    "src/a.rs",
                    vec![error(1, 10, "cannot find function `secret`")],
                ),
                text.to_string(),
            ),
        )]);
        let sources = HashMap::from([("src/b.rs".to_string(), text.to_string())]);
        let mut reports = vec![report(
            "src/b.rs",
            vec![error(1, 10, "cannot find function `secret`")],
        )];
        set_aside_carried(&mut reports, &sources, &baselines);
        assert_eq!(reports[0].errors, 1);
    }
}
