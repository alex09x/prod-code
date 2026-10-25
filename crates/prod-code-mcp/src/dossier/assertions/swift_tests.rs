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

#[test]
fn coverage_swift_diagnostic_variants() {
    for (text, format, expression) in [
        (
            "XCTAssertEqual failed: (\"α\") is not equal to (\"β\")",
            "XCTAssertEqual",
            "left == right",
        ),
        (
            "XCTAssertNotEqual failed: (\"α\") is equal to (\"β\")",
            "XCTAssertNotEqual",
            "left != right",
        ),
    ] {
        let got = parse_swift_xctest_assertion(&[text], &[text.into()])
            .flatten()
            .unwrap();
        assert_eq!(
            got,
            AssertionEvidence {
                format: format.into(),
                expression: Some(expression.into()),
                actual: None,
                expected: None,
                left: Some("α".into()),
                right: Some("β".into()),
                operands: vec!["α".into(), "β".into()],
                excerpt: text.into()
            }
        );
    }
    for (expression, left, right) in [
        ("(lhs → α) != (rhs → β)", Some("α"), Some("β")),
        ("(1) == (2)", Some("1"), Some("2")),
        ("ready", None, None),
    ] {
        let text = format!("Expectation failed: {expression}");
        let got = parse_swift_testing_assertion(&[&text], std::slice::from_ref(&text))
            .flatten()
            .unwrap();
        assert_eq!(
            got,
            AssertionEvidence {
                format: "swift-testing".into(),
                expression: Some(expression.into()),
                actual: None,
                expected: None,
                left: left.map(str::to_string),
                right: right.map(str::to_string),
                operands: match (left, right) {
                    (Some(l), Some(r)) => vec![l.into(), r.into()],
                    _ => vec![expression.into()],
                },
                excerpt: text
            }
        );
    }
    for text in [
        "",
        "XCTAssertEqual failed: (\"1\")",
        "XCTAssertNotEqual failed: (\"1\") is equal to (\"2",
        "ordinary output",
    ] {
        assert_eq!(parse_swift_xctest_assertion(&[text], &[text.into()]), None);
        assert_eq!(parse_swift_testing_assertion(&[text], &[text.into()]), None);
    }
}
