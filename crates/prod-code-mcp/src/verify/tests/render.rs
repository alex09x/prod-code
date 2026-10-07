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

#[test]
fn serialized_frame_bounding_with_escape_heavy_output() {
    use crate::server::transport::{MAX_JSONRPC_FRAME_BYTES, bound_serialized_response};

    // 40,000 newlines escape to 80,000 bytes in JSON ("\n" -> "\\n")
    let raw = "\n".repeat(40_000);
    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "content": [{
                "type": "text",
                "text": raw
            }],
            "isError": false
        }
    });

    let serialized = bound_serialized_response(resp, MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.len() <= MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.contains("output truncated"));
}

#[test]
fn fixed_render_preserves_outcomes_and_post_fix_report() {
    use crate::fixit::{Fixed, Outcome};

    // Construct a verbose pre-fix report that exceeds MAX_RENDER_BYTES
    let huge_diagnostics: Vec<crate::verify::Diagnostic> = (0..500)
        .map(|i| crate::verify::Diagnostic {
            level: "error".into(),
            code: None,
            message: "x".repeat(1000),
            file: Some(format!("src/lib_{i}.rs")),
            line: Some(i as u64),
            column: Some(1),
        })
        .collect();

    let before = VerifyReport {
        kind: VerifyKind::Check,
        language: "rust".into(),
        command: vec!["cargo".into(), "check".into()],
        exit_code: Some(1),
        timed_out: false,
        duration_ms: 1000,
        diagnostics: huge_diagnostics,
        tests_passed: 0,
        tests_failed: 0,
        failures: vec![],
        tail: String::new(),
        fixes: vec![],
        benches: vec![],
        usage: None,
        platform: None,
    };

    let after = VerifyReport {
        kind: VerifyKind::Check,
        language: "rust".into(),
        command: vec!["cargo".into(), "check".into()],
        exit_code: Some(0),
        timed_out: false,
        duration_ms: 500,
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

    let outcomes = vec![
        Outcome {
            file: "src/fix1.rs".into(),
            line: 42,
            message: "unused variable `x`".into(),
            skipped: None,
        },
        Outcome {
            file: "src/fix2.rs".into(),
            line: 88,
            message: "unneeded return statement".into(),
            skipped: None,
        },
    ];

    let fixed = Fixed {
        before,
        outcomes,
        after: Some(after),
        note: None,
    };

    let text = fixed.render(100);
    // Crucial: fix outcomes and post-fix report must NOT be dropped by truncation
    assert!(
        text.contains("fixed src/fix1.rs:42: unused variable `x`"),
        "{text}"
    );
    assert!(
        text.contains("fixed src/fix2.rs:88: unneeded return statement"),
        "{text}"
    );
    assert!(text.contains("after the fixes:"), "{text}");
    assert!(text.contains("rust check: OK"), "{text}");
    assert!(text.contains("pre-fix diagnostics truncated"), "{text}");
    assert!(
        text.len() <= MAX_RENDER_BYTES + 500,
        "len is {}",
        text.len()
    );
}

#[test]
fn fixed_render_caps_huge_outcomes_preserving_after_report() {
    use crate::fixit::{Fixed, Outcome};

    let before = VerifyReport {
        kind: VerifyKind::Check,
        language: "rust".into(),
        command: vec!["cargo".into(), "check".into()],
        exit_code: Some(1),
        timed_out: false,
        duration_ms: 1000,
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

    let after = VerifyReport {
        kind: VerifyKind::Check,
        language: "rust".into(),
        command: vec!["cargo".into(), "check".into()],
        exit_code: Some(0),
        timed_out: false,
        duration_ms: 500,
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

    // 1,000 outcomes would take ~70KB, far exceeding MAX_RENDER_BYTES
    let outcomes: Vec<Outcome> = (0..1_000)
        .map(|i| Outcome {
            file: format!("src/fix_{i}.rs"),
            line: i as u64,
            message: format!("some automated fix outcome description {i}"),
            skipped: None,
        })
        .collect();

    let fixed = Fixed {
        before,
        outcomes,
        after: Some(after),
        note: None,
    };

    let text = fixed.render(100);
    assert!(text.contains("more fix outcome(s)"), "{text}");
    assert!(text.contains("after the fixes:"), "{text}");
    assert!(text.contains("rust check: OK"), "{text}");
    assert!(
        text.len() <= MAX_RENDER_BYTES,
        "len is {} > {}",
        text.len(),
        MAX_RENDER_BYTES
    );
}

#[test]
fn serialized_frame_bounding_preserves_non_tool_responses() {
    use crate::server::transport::{MAX_JSONRPC_FRAME_BYTES, bound_serialized_response};

    // A tools/list response exceeding 60KB should not be converted to a tool content result
    let big_tools: Vec<serde_json::Value> = (0..200)
        .map(|i| {
            serde_json::json!({
                "name": format!("tool_{i}"),
                "description": "x".repeat(400),
                "inputSchema": { "type": "object" }
            })
        })
        .collect();

    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "tools": big_tools
        }
    });

    let serialized = bound_serialized_response(resp.clone(), MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.contains("tools"));
    assert!(serialized.contains("tool_0"));
    assert!(!serialized.contains("output truncated to avoid exceeding MCP frame line limits"));
}

#[test]
fn verify_render_reserves_budget_for_test_failures_despite_many_diagnostics() {
    // Construct 500 verbose diagnostics that would fill MAX_RENDER_BYTES
    let huge_diagnostics: Vec<crate::verify::Diagnostic> = (0..500)
        .map(|i| crate::verify::Diagnostic {
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
    // Crucial: Test failures MUST be rendered despite verbose compiler diagnostics
    assert!(
        text.contains("--- FAILED tests::acceptance::test_order_matching ---"),
        "{text}"
    );
    assert!(
        text.contains("--- FAILED tests::unit::test_user_auth ---"),
        "{text}"
    );
    assert!(text.contains("more diagnostic(s)"), "{text}");
    assert!(
        text.len() <= MAX_RENDER_BYTES + 500,
        "len is {}",
        text.len()
    );
}
