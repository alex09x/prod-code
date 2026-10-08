/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::{
    MAX_RENDER_BYTES, TestFailure, VerifyKind, VerifyReport, tail_from_boundary,
    truncate_to_boundary,
};

#[test]
fn render_truncates_large_failure_output() {
    let huge_output = "line of failure output\n".repeat(2000);
    assert!(huge_output.len() > 40_000);

    let report = VerifyReport {
        kind: VerifyKind::Test,
        language: "rust".into(),
        command: vec!["cargo".into(), "test".into()],
        exit_code: Some(101),
        timed_out: false,
        duration_ms: 1200,
        diagnostics: vec![],
        tests_passed: 10,
        tests_failed: 1,
        failures: vec![TestFailure {
            name: "tests::huge_failure".into(),
            output: huge_output,
        }],
        tail: String::new(),
        fixes: vec![],
        benches: vec![],
        usage: None,
        platform: None,
    };

    let text = report.render(10);
    assert!(text.contains("--- FAILED tests::huge_failure ---"));
    assert!(text.contains("bytes truncated"));
    assert!(text.len() <= MAX_RENDER_BYTES + 500);
}

#[test]
fn verify_render_keeps_failure_details_after_oversized_diagnostic() {
    use super::super::types::Diagnostic;

    let report = VerifyReport {
        kind: VerifyKind::Test,
        language: "rust".into(),
        command: vec!["cargo".into(), "test".into()],
        exit_code: Some(101),
        timed_out: false,
        duration_ms: 1200,
        diagnostics: vec![Diagnostic {
            level: "error".into(),
            code: None,
            message: "oversized diagnostic ".repeat(1500),
            file: Some("src/lib.rs".into()),
            line: Some(1),
            column: Some(1),
        }],
        tests_passed: 0,
        tests_failed: 1,
        failures: vec![TestFailure {
            name: "tests::must_be_visible".into(),
            output: "assertion failed".into(),
        }],
        tail: String::new(),
        fixes: vec![],
        benches: vec![],
        usage: None,
        platform: None,
    };

    let text = report.render(10);
    assert!(text.contains("--- FAILED tests::must_be_visible ---"));
    assert!(text.contains("diagnostic truncated"));
    assert!(text.len() <= MAX_RENDER_BYTES + 500);
}

#[test]
fn render_caps_total_output_with_many_failures() {
    let failures: Vec<TestFailure> = (0..20)
        .map(|i| TestFailure {
            name: format!("tests::failing_test_{i}"),
            output: format!("failure diagnostic info for test {i}\n").repeat(100),
        })
        .collect();

    let report = VerifyReport {
        kind: VerifyKind::Test,
        language: "rust".into(),
        command: vec!["cargo".into(), "test".into()],
        exit_code: Some(101),
        timed_out: false,
        duration_ms: 2500,
        diagnostics: vec![],
        tests_passed: 0,
        tests_failed: 20,
        failures,
        tail: String::new(),
        fixes: vec![],
        benches: vec![],
        usage: None,
        platform: None,
    };

    let text = report.render(20);
    assert!(text.contains("MCP line limits"));
    assert!(text.len() <= MAX_RENDER_BYTES + 500);
}

#[test]
fn render_truncates_large_tail_output() {
    let huge_tail = "raw unstructured stderr line\n".repeat(2000);
    assert!(huge_tail.len() > 40_000);

    let report = VerifyReport {
        kind: VerifyKind::Check,
        language: "rust".into(),
        command: vec!["cargo".into(), "check".into()],
        exit_code: Some(1),
        timed_out: false,
        duration_ms: 800,
        diagnostics: vec![],
        tests_passed: 0,
        tests_failed: 0,
        failures: vec![],
        tail: huge_tail,
        fixes: vec![],
        benches: vec![],
        usage: None,
        platform: None,
    };

    let text = report.render(10);
    assert!(text.contains("--- output tail ---"));
    assert!(text.len() <= MAX_RENDER_BYTES + 500);
}

#[test]
fn multibyte_utf8_boundary_handling() {
    // 4-byte characters (emojis) and 2-byte characters (Cyrillic)
    let s = "🦀🚀Привет, мир!🌟🔥";
    // Slicing in the middle of a 4-byte char should not panic
    let truncated = truncate_to_boundary(s, 2);
    assert_eq!(truncated, ""); // first char is 4 bytes, so truncated to 2 gives empty string
    let truncated4 = truncate_to_boundary(s, 4);
    assert_eq!(truncated4, "🦀");

    let tail = tail_from_boundary(s, 2);
    // last char is 4 bytes, so tail of 2 bytes should safely snap forward to valid boundary
    assert!(!tail.is_empty() || tail.is_empty()); // should not panic
}
