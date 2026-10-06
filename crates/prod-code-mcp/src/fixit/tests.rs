/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::parse::parse_fixes;
use super::plan::plan;
use super::types::{Edit, Fix, Fixed, Outcome};

// An unused import, as cargo reports it: the suggestion is in a child.
const UNUSED: &str = r#"{"reason":"compiler-message","message":{"level":"warning","code":{"code":"unused_imports"},"message":"unused import: `std::fmt`","spans":[{"file_name":"src/lib.rs","byte_start":4,"byte_end":12,"line_start":1,"is_primary":true,"text":[{"text":"use std::fmt;"}],"suggested_replacement":null,"suggestion_applicability":null}],"children":[{"message":"remove the whole `use` item","spans":[{"file_name":"src/lib.rs","byte_start":0,"byte_end":14,"line_start":1,"is_primary":true,"text":[{"text":"use std::fmt;"}],"suggested_replacement":"","suggestion_applicability":"MachineApplicable"}],"children":[]}]}}"#;

#[test]
fn only_machine_applicable_suggestions_are_taken() {
    let fixes = parse_fixes(UNUSED);
    assert_eq!(fixes.len(), 1);
    assert_eq!(
        fixes[0].message,
        "unused import: `std::fmt`: remove the whole `use` item"
    );
    assert_eq!((fixes[0].edits[0].start, fixes[0].edits[0].end), (0, 14));
    let maybe = UNUSED.replace("MachineApplicable", "MaybeIncorrect");
    assert!(parse_fixes(&maybe).is_empty());
    assert!(parse_fixes(r#"{"reason":"build-finished","success":true}"#).is_empty());
    assert!(parse_fixes("not json").is_empty());
}

#[test]
fn a_fix_is_applied_whole_once_and_only_to_the_text_it_was_made_for() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "use std::fmt;\nfn f() {}\n").unwrap();
    let fix = parse_fixes(UNUSED).remove(0);
    // Reported twice (lib and test targets), applied once.
    let (files, outcomes) = plan(dir.path(), &[fix.clone(), fix.clone()]);
    assert_eq!(files[&dir.path().join("src/lib.rs")], "fn f() {}\n");
    assert_eq!(outcomes.len(), 1);
    assert!(outcomes[0].skipped.is_none());

    // Two parts of one suggestion go in together; an overlapping second fix is skipped.
    let parens = Fix {
        edits: vec![
            Edit {
                start: 3,
                end: 3,
                replacement: "(".into(),
                ..fix.edits[0].clone()
            },
            Edit {
                start: 12,
                end: 12,
                replacement: ")".into(),
                ..fix.edits[0].clone()
            },
        ],
        ..fix.clone()
    };
    let (files, outcomes) = plan(dir.path(), &[parens, fix.clone()]);
    assert_eq!(
        files[&dir.path().join("src/lib.rs")],
        "use( std::fmt);\nfn f() {}\n"
    );
    assert_eq!(
        outcomes[1].skipped.as_deref(),
        Some("it overlaps a fix already taken")
    );

    // The line is not what the compiler saw, or the file is not the workspace's.
    std::fs::write(dir.path().join("src/lib.rs"), "use std::io;\nfn f() {}\n").unwrap();
    let (files, outcomes) = plan(dir.path(), std::slice::from_ref(&fix));
    assert!(files.is_empty());
    assert_eq!(
        outcomes[0].skipped.as_deref(),
        Some("the file changed since the compiler read it")
    );
    let outside = Fix {
        edits: vec![Edit {
            file: "/registry/src/x.rs".into(),
            ..fix.edits[0].clone()
        }],
        ..fix
    };
    let (_, outcomes) = plan(dir.path(), &[outside]);
    assert!(
        outcomes[0]
            .skipped
            .as_deref()
            .unwrap()
            .contains("not a file of this workspace")
    );
}

#[test]
fn the_report_says_what_was_fixed_skipped_and_left() {
    let report = |exit: i32| crate::verify::VerifyReport {
        kind: crate::verify::VerifyKind::Lint,
        language: "rust".into(),
        command: vec!["cargo".into(), "clippy".into()],
        exit_code: Some(exit),
        timed_out: false,
        duration_ms: 100,
        diagnostics: vec![],
        tests_passed: 0,
        tests_failed: 0,
        failures: vec![],
        tail: String::new(),
        fixes: vec![],
        benches: vec![],
        usage: None,
        platform: None,
    };
    let outcome = |skipped: Option<&str>| Outcome {
        file: "src/lib.rs".into(),
        line: 1,
        message: "unused import".into(),
        skipped: skipped.map(str::to_string),
    };
    let fixed = Fixed {
        before: report(101),
        outcomes: vec![
            outcome(None),
            outcome(Some("it overlaps a fix already taken")),
        ],
        after: Some(report(0)),
        note: None,
    };
    let text = fixed.render(10);
    for expected in [
        "machine-applicable fixes: 1 applied, 1 skipped",
        "  fixed src/lib.rs:1: unused import",
        "  skipped src/lib.rs:1: unused import (it overlaps a fix already taken)",
        "after the fixes:",
    ] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
    assert!(fixed.ok());
    let nothing = Fixed {
        after: None,
        ..fixed
    };
    assert!(!nothing.ok() && !nothing.render(10).contains("after the fixes"));
    // A linter that fixes by itself: the note replaces the count of compiler fixes.
    let by_tool = Fixed {
        note: Some("fixes: `ruff check --fix` rewrote 1 file(s)".into()),
        ..nothing
    };
    let text = by_tool.render(10);
    assert!(
        text.contains("`ruff check --fix` rewrote 1 file(s)"),
        "{text}"
    );
    assert!(!text.contains("machine-applicable fixes"), "{text}");
}
