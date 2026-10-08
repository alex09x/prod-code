/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::{Diagnostic, MAX_RENDER_BYTES, TestFailure, VerifyKind, VerifyReport};

#[test]
fn verify_render_keeps_failure_details_after_oversized_diagnostic() {
    let report = VerifyReport {
        kind: VerifyKind::Test,
        language: "rust".into(),
        command: vec!["cargo".into(), "test".into()],
        exit_code: Some(101),
        timed_out: false,
        duration_ms: 1000,
        diagnostics: vec![Diagnostic {
            level: "warning".into(),
            code: None,
            message: "d".repeat(MAX_RENDER_BYTES),
            file: Some("src/lib.rs".into()),
            line: Some(1),
            column: Some(1),
        }],
        tests_passed: 0,
        tests_failed: 1,
        failures: vec![TestFailure {
            name: "tests::must_be_visible".into(),
            output: "failure detail".into(),
        }],
        tail: String::new(),
        fixes: vec![],
        benches: vec![],
        usage: None,
        platform: None,
    };

    let rendered = report.render(10);

    assert!(
        rendered.contains("--- FAILED tests::must_be_visible ---"),
        "oversized diagnostics must not hide the failing test details"
    );
    assert!(rendered.len() <= MAX_RENDER_BYTES + 500);
}

#[test]
fn verify_render_reserves_budget_for_test_failures_despite_many_diagnostics() {
    let huge_diagnostics: Vec<Diagnostic> = (0..500)
        .map(|i| Diagnostic {
            level: "warning".into(),
            code: None,
            message: "w".repeat(500),
            file: Some(format!("src/warn_{i}.rs")),
            line: Some(i as u64),
            column: Some(1),
        })
        .collect();
    let failures = vec![
        TestFailure {
            name: "tests::acceptance::test_order_matching".into(),
            output: "assertion failed: `(left == right)`\n  left: `0`\n right: `1`".into(),
        },
        TestFailure {
            name: "tests::unit::test_user_auth".into(),
            output: "panic at src/auth.rs:42: token expired".into(),
        },
    ];
    let report = VerifyReport {
        kind: VerifyKind::Test,
        language: "rust".into(),
        command: vec!["cargo".into(), "test".into()],
        exit_code: Some(101),
        timed_out: false,
        duration_ms: 1200,
        diagnostics: huge_diagnostics,
        tests_passed: 10,
        tests_failed: 2,
        failures,
        tail: String::new(),
        fixes: vec![],
        benches: vec![],
        usage: None,
        platform: None,
    };

    let text = report.render(40);
    assert!(text.contains("--- FAILED tests::acceptance::test_order_matching ---"));
    assert!(text.contains("--- FAILED tests::unit::test_user_auth ---"));
    assert!(text.contains("more diagnostic(s)"));
    assert!(text.len() <= MAX_RENDER_BYTES + 500);
}

#[test]
fn verify_render_reports_failures_omitted_by_byte_budget() {
    let failures: Vec<TestFailure> = (0..10)
        .map(|i| TestFailure {
            name: format!("tests::test_{i}"),
            output: "f".repeat(3500),
        })
        .collect();
    let report = VerifyReport {
        kind: VerifyKind::Test,
        language: "rust".into(),
        command: vec!["cargo".into(), "test".into()],
        exit_code: Some(101),
        timed_out: false,
        duration_ms: 1000,
        diagnostics: vec![],
        tests_passed: 0,
        tests_failed: 10,
        failures,
        tail: String::new(),
        fixes: vec![],
        benches: vec![],
        usage: None,
        platform: None,
    };

    let text = report.render(10);
    assert!(text.contains("more failed test(s)"));
    assert!(text.len() <= MAX_RENDER_BYTES + 500);
}
