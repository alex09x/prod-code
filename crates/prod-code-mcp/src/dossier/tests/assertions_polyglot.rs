/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::dossier::assertions::parse_assertion_evidence;
use crate::dossier::types::{AssertionEvidence, FailureDossier, FailureSite, Suspect};

#[test]
fn parses_pytest_assertions() {
    let pytest = "def test_f():\n>       assert result == expected\nE       assert 1 == 2\n\ntest_f.py:2: AssertionError\n";
    let ev = parse_assertion_evidence(pytest).expect("parsed pytest");
    assert_eq!(ev.format, "pytest/assert_eq");
    assert_eq!(ev.left.as_deref(), Some("1"));
    assert_eq!(ev.right.as_deref(), Some("2"));
    assert_eq!(ev.expression.as_deref(), Some("assert 1 == 2"));

    let pytest_in = "def test_f():\n>       assert 'a' in 'xyz'\nE       assert 'a' in 'xyz'\n\ntest_f.py:2: AssertionError\n";
    let ev_in = parse_assertion_evidence(pytest_in).expect("parsed pytest in");
    assert_eq!(ev_in.format, "pytest/assert_in");
    assert_eq!(ev_in.left.as_deref(), Some("'a'"));
    assert_eq!(ev_in.right.as_deref(), Some("'xyz'"));
}

#[test]
fn parses_unittest_assertions() {
    let unittest = "Traceback (most recent call last):\n  File \"test_x.py\", line 5, in test_foo\n    self.assertEqual(a, b)\nAssertionError: 1 != 2\n";
    let ev = parse_assertion_evidence(unittest).expect("parsed unittest");
    assert_eq!(ev.format, "unittest/assertEqual");
    assert_eq!(ev.left.as_deref(), Some("1"));
    assert_eq!(ev.right.as_deref(), Some("2"));

    let unittest_bool = "Traceback (most recent call last):\n  File \"test_x.py\", line 5, in test_bar\n    self.assertTrue(False)\nAssertionError: False is not true\n";
    let ev_bool = parse_assertion_evidence(unittest_bool).expect("parsed unittest assertTrue");
    assert_eq!(ev_bool.format, "unittest/assertTrue");
    assert_eq!(ev_bool.actual.as_deref(), Some("False"));
    assert_eq!(ev_bool.expected.as_deref(), Some("True"));
}

#[test]
fn parses_go_assertions() {
    let testify = "    foo_test.go:12:\n        \tError:      \tNot equal:\n        \t            \texpected: 1\n        \t            \tactual  : 2\n        \tTest:       \tTestFoo\n";
    let ev = parse_assertion_evidence(testify).expect("parsed testify");
    assert_eq!(ev.format, "testify/assert");
    assert_eq!(ev.actual.as_deref(), Some("2"));
    assert_eq!(ev.expected.as_deref(), Some("1"));

    let got_want = "    bar_test.go:20: got: 42, want: 100\n";
    let ev2 = parse_assertion_evidence(got_want).expect("parsed go got/want");
    assert_eq!(ev2.format, "go/got_want");
    assert_eq!(ev2.actual.as_deref(), Some("42"));
    assert_eq!(ev2.expected.as_deref(), Some("100"));
}

#[test]
fn parses_swift_assertions() {
    let xctest = "Test Case '-[FooTests testBar]' started.\n/path/FooTests.swift:15: error: -[FooTests testBar] : XCTAssertEqual failed: (\"hello\") is not equal to (\"world\")\n";
    let ev = parse_assertion_evidence(xctest).expect("parsed XCTest");
    assert_eq!(ev.format, "XCTAssertEqual");
    assert_eq!(ev.left.as_deref(), Some("hello"));
    assert_eq!(ev.right.as_deref(), Some("world"));

    let swift_testing = "Expectation failed: (count → 0) == (expected → 5)\n";
    let ev2 = parse_assertion_evidence(swift_testing).expect("parsed swift-testing");
    assert_eq!(ev2.format, "swift-testing");
    assert_eq!(ev2.left.as_deref(), Some("0"));
    assert_eq!(ev2.right.as_deref(), Some("5"));
}

#[test]
fn parses_cpp_assertions() {
    let gtest = "foo_test.cc:10: Failure\nExpected equality of these values:\n  x\n    Which is: 10\n  y\n    Which is: 20\n";
    let ev = parse_assertion_evidence(gtest).expect("parsed gtest");
    assert_eq!(ev.format, "gtest/EXPECT_EQ");
    assert_eq!(ev.left.as_deref(), Some("10"));
    assert_eq!(ev.right.as_deref(), Some("20"));

    let catch2 = "CHECK( result == 42 )\nwith expansion:\n  0 == 42\n";
    let ev2 = parse_assertion_evidence(catch2).expect("parsed catch2");
    assert_eq!(ev2.format, "catch2");
    assert_eq!(ev2.left.as_deref(), Some("0"));
    assert_eq!(ev2.right.as_deref(), Some("42"));
    assert_eq!(ev2.expression.as_deref(), Some("result == 42"));
}

#[test]
fn failure_dossier_serializes_roadmap_fields() {
    let dossier = FailureDossier {
        test: "test_failure".to_string(),
        panic_line: Some(42),
        expression: Some("left == right".to_string()),
        output: "test failed".to_string(),
        sites: vec![],
        assertion: Some(AssertionEvidence {
            format: "assert_eq".to_string(),
            expression: Some("left == right".to_string()),
            actual: None,
            expected: None,
            left: Some("1".to_string()),
            right: Some("2".to_string()),
            operands: vec!["1".to_string(), "2".to_string()],
            excerpt: "assertion `left == right` failed".to_string(),
        }),
        suspects: vec![Suspect {
            function: "do_something".to_string(),
            file: "src/lib.rs".to_string(),
            line: 10,
            hops: 1,
            diff: None,
        }],
    };
    let json = serde_json::to_value(&dossier).expect("serialize dossier");
    assert_eq!(
        json.get("failing_test").and_then(|v| v.as_str()),
        Some("test_failure")
    );
    assert_eq!(
        json.get("test").and_then(|v| v.as_str()),
        Some("test_failure")
    );
    assert_eq!(json.get("panic_line").and_then(|v| v.as_u64()), Some(42));
    assert_eq!(
        json.get("expression").and_then(|v| v.as_str()),
        Some("left == right")
    );
    assert!(json.get("runtime_values").is_some());
    assert!(json.get("suspect_recent_changes").is_some());
}

#[test]
fn failure_dossier_json_round_trip() {
    let dossier = FailureDossier {
        test: "test_roundtrip".to_string(),
        panic_line: Some(99),
        expression: Some("a == b".to_string()),
        output: "failure details".to_string(),
        sites: vec![FailureSite {
            file: "src/lib.rs".to_string(),
            line: 99,
            snippet: "> 99 | assert_eq!(a, b)".to_string(),
            function: Some("test_roundtrip".to_string()),
            callers: vec!["main".to_string()],
            diff: Some("+ diff".to_string()),
        }],
        assertion: Some(AssertionEvidence {
            format: "assert_eq".to_string(),
            expression: Some("left == right".to_string()),
            actual: None,
            expected: None,
            left: Some("foo".to_string()),
            right: Some("bar".to_string()),
            operands: vec!["foo".to_string(), "bar".to_string()],
            excerpt: "assertion `left == right` failed".to_string(),
        }),
        suspects: vec![Suspect {
            function: "helper".to_string(),
            file: "src/util.rs".to_string(),
            line: 12,
            hops: 1,
            diff: None,
        }],
    };

    let json_str = serde_json::to_string(&dossier).expect("serialize");
    let deserialized: FailureDossier = serde_json::from_str(&json_str).expect("deserialize");
    assert_eq!(dossier, deserialized);

    // Also test deserializing from legacy JSON without aliases
    let legacy_json = serde_json::json!({
        "test": "test_legacy",
        "output": "out",
        "sites": [],
        "suspects": [],
        "assertion": null
    });
    let from_legacy: FailureDossier = serde_json::from_value(legacy_json).expect("from legacy");
    assert_eq!(from_legacy.test, "test_legacy");

    // Also test deserializing from roadmap-only JSON
    let roadmap_json = serde_json::json!({
        "failing_test": "test_roadmap",
        "output": "out",
        "sites": [],
        "suspect_recent_changes": [],
        "runtime_values": null,
        "panic_line": 50,
        "expression": "x > 0"
    });
    let from_roadmap: FailureDossier = serde_json::from_value(roadmap_json).expect("from roadmap");
    assert_eq!(from_roadmap.test, "test_roadmap");
    assert_eq!(from_roadmap.panic_line, Some(50));
    assert_eq!(from_roadmap.expression.as_deref(), Some("x > 0"));
}

#[test]
fn rejects_unrelated_logs_and_explicit_panics() {
    assert_eq!(
        parse_assertion_evidence("thread 'x' panicked at 'explicit panic'"),
        None
    );
    assert_eq!(
        parse_assertion_evidence("assertion failed: flag_is_true"),
        None
    );
    assert_eq!(
        parse_assertion_evidence("[INFO] checking assert condition: ok"),
        None
    );
    assert_eq!(
        parse_assertion_evidence("assertion `left == right` failed"),
        None
    );
    assert_eq!(
        parse_assertion_evidence("TypeError: undefined is not a function"),
        None
    );
}

#[test]
fn does_not_collect_across_test_boundaries() {
    let output = "---- tests::test_one stdout ----\nthread 'tests::test_one' panicked at src/lib.rs:10:5:\nassertion `left == right` failed\n  left: 1\n right: 2\n\n---- tests::test_two stdout ----\nthread 'tests::test_two' panicked at src/lib.rs:20:5:\nassertion `left == right` failed\n  left: 10\n right: 20\n";
    let ev = parse_assertion_evidence(output).expect("parsed assertion");
    assert_eq!(ev.left.as_deref(), Some("1"));
    assert_eq!(ev.right.as_deref(), Some("2"));
    assert!(!ev.excerpt.contains("test_two"));
}

#[test]
fn render_compact_produces_clean_evidence() {
    let ev = AssertionEvidence {
        format: "assert_eq".to_string(),
        expression: Some("left == right".to_string()),
        actual: Some("4".to_string()),
        expected: Some("5".to_string()),
        left: Some("4".to_string()),
        right: Some("5".to_string()),
        operands: vec!["4".to_string(), "5".to_string()],
        excerpt: "assertion `left == right` failed\n  left: 4\n right: 5".to_string(),
    };
    let compact = ev.render_compact();
    assert_eq!(
        compact,
        "assertion [assert_eq (left == right)]: actual: 4, expected: 5\n"
    );
}
