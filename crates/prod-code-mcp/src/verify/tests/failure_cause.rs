/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A failed run always shows what failed it (#1230): the compiler error even after many
//! warnings, the failed test, or the output tail when nothing structured explains it.

use super::super::types::{Diagnostic, MAX_RENDER_BYTES, TestFailure, VerifyKind, VerifyReport};

fn diagnostic(level: &str, message: &str, line: u64) -> Diagnostic {
    Diagnostic {
        level: level.into(),
        code: None,
        message: message.into(),
        file: Some("src/lib.rs".into()),
        line: Some(line),
        column: Some(1),
    }
}

fn warnings(count: usize, message_len: usize) -> Vec<Diagnostic> {
    (0..count)
        .map(|i| {
            diagnostic(
                "warning",
                &format!("unused {i} {}", "w".repeat(message_len)),
                i as u64,
            )
        })
        .collect()
}

fn report(
    exit: Option<i32>,
    diagnostics: Vec<Diagnostic>,
    failures: Vec<TestFailure>,
) -> VerifyReport {
    VerifyReport {
        kind: VerifyKind::Test,
        language: "rust".into(),
        command: vec!["cargo".into(), "test".into()],
        exit_code: exit,
        timed_out: false,
        duration_ms: 100,
        diagnostics,
        tests_passed: 0,
        tests_failed: failures.len() as u64,
        failures,
        tail: "warning: `demo` generated 92 warnings\nerror: linking with `cc` failed: exit status: 1\n"
            .into(),
        fixes: vec![],
        benches: vec![],
        usage: None,
        platform: None,
    }
}

fn with(mut base: Vec<Diagnostic>, at: usize, extra: Diagnostic) -> Vec<Diagnostic> {
    base.insert(at, extra);
    base
}

/// (case, report, max_items, must contain, must not contain)
type Case = (
    &'static str,
    VerifyReport,
    usize,
    Vec<&'static str>,
    Vec<&'static str>,
);

#[test]
fn a_failed_run_shows_its_cause_within_the_item_and_byte_caps() {
    let error = diagnostic("error", "mismatched types", 500);
    let second = diagnostic("error", "cannot find value `x`", 501);
    let failure = TestFailure {
        name: "tests::focused".into(),
        output: "assertion failed: left == right".into(),
    };
    let cases: Vec<Case> = vec![
        (
            "one error after 92 warnings, code_test cap",
            report(Some(101), with(warnings(92, 10), 92, error.clone()), vec![]),
            40,
            vec![
                "error: mismatched types (src/lib.rs:500:1)",
                "... 53 more diagnostic(s)",
            ],
            vec!["--- output tail ---"],
        ),
        (
            "one error after 250 warnings, CLI cap",
            report(
                Some(101),
                with(warnings(250, 10), 250, error.clone()),
                vec![],
            ),
            200,
            vec!["error: mismatched types", "... 51 more diagnostic(s)"],
            vec!["--- output tail ---"],
        ),
        (
            "one error after warnings that fill the byte budget",
            report(
                Some(101),
                with(warnings(200, 400), 200, error.clone()),
                vec![],
            ),
            200,
            vec!["error: mismatched types", "more diagnostic(s)"],
            vec!["--- output tail ---"],
        ),
        (
            "two errors between warnings keep their order",
            report(
                Some(101),
                with(
                    with(warnings(60, 10), 30, error.clone()),
                    61,
                    second.clone(),
                ),
                vec![],
            ),
            40,
            vec!["error: mismatched types", "error: cannot find value `x`"],
            vec![],
        ),
        (
            "only warnings and no failed test: the tail explains the exit",
            report(Some(101), warnings(92, 10), vec![]),
            40,
            vec!["--- output tail ---", "linking with `cc` failed"],
            vec![],
        ),
        (
            "only warnings filling the byte budget still leave room for the tail",
            report(Some(101), warnings(200, 400), vec![]),
            200,
            vec!["--- output tail ---", "linking with `cc` failed"],
            vec![],
        ),
        (
            "a failed test after warnings needs no tail",
            report(Some(101), warnings(92, 10), vec![failure.clone()]),
            40,
            vec!["--- FAILED tests::focused ---", "assertion failed"],
            vec!["--- output tail ---"],
        ),
        (
            "a passing run with warnings shows no tail",
            report(Some(0), warnings(92, 10), vec![]),
            40,
            vec!["rust test: OK", "... 52 more diagnostic(s)"],
            vec!["--- output tail ---"],
        ),
        (
            "a failed run with nothing parsed shows the tail as before",
            report(Some(101), vec![], vec![]),
            40,
            vec!["--- output tail ---", "linking with `cc` failed"],
            vec![],
        ),
    ];
    for (case, report, max_items, present, absent) in cases {
        let text = report.render(max_items);
        for needle in present {
            assert!(
                text.contains(needle),
                "{case}: missing {needle:?} in\n{text}"
            );
        }
        for needle in absent {
            assert!(
                !text.contains(needle),
                "{case}: unexpected {needle:?} in\n{text}"
            );
        }
        assert!(
            text.len() <= MAX_RENDER_BYTES + 500,
            "{case}: {} bytes",
            text.len()
        );
        if report.errors() > 0 {
            let first = text.lines().nth(2).unwrap_or_default();
            assert!(
                first.starts_with("  error: "),
                "{case}: errors come first, got {first:?}"
            );
        }
    }
}

#[test]
fn cargo_summaries_of_a_failed_run_are_not_compiler_errors() {
    let stderr = "warning: unused variable: `x`\n --> src/lib.rs:1:22\n\
        error: test failed, to rerun pass `--test it`\n\nCaused by:\n  \
        process didn't exit successfully: `target/debug/deps/it-0` (signal: 6, SIGABRT: process abort signal)\n\
        error: could not compile `demo` (test \"it\") due to 1 previous error\n\
        error[E0308]: mismatched types\n --> tests/it.rs:2:29\n";
    let parsed = crate::verify::parse::rust::parse_rustc_text(stderr);
    let levels: Vec<(&str, &str)> = parsed
        .iter()
        .map(|d| (d.level.as_str(), d.message.as_str()))
        .collect();
    assert_eq!(
        levels,
        [
            ("warning", "unused variable: `x`"),
            ("error", "mismatched types")
        ]
    );
}

#[test]
fn errors_come_first_and_each_level_keeps_compiler_order() {
    let diagnostics = vec![
        diagnostic("warning", "w0", 1),
        diagnostic("error", "e0", 2),
        diagnostic("warning", "w1", 3),
        diagnostic("error", "e1", 4),
        diagnostic("note", "n0", 5),
    ];
    let mut out = String::new();
    let rendered = super::super::render::append_diagnostics(&mut out, &diagnostics, 10, 10_000);
    assert_eq!(rendered, 5);
    let messages: Vec<&str> = out
        .lines()
        .map(|line| line.trim_start().split(" (").next().unwrap())
        .collect();
    assert_eq!(
        messages,
        [
            "error: e0",
            "error: e1",
            "warning: w0",
            "warning: w1",
            "note: n0"
        ]
    );
}
