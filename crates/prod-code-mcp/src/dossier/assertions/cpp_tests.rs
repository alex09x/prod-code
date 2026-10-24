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

fn parse(text: &str, catch: bool) -> Option<AssertionEvidence> {
    let raw: Vec<_> = text.lines().collect();
    let stripped: Vec<_> = raw.iter().map(|s| s.to_string()).collect();
    if catch {
        parse_catch2_assertion(&raw, &stripped).flatten()
    } else {
        parse_gtest_assertion(&raw, &stripped).flatten()
    }
}

#[test]
fn coverage_gtest_values_and_refusals() {
    let text =
        "Expected equality of these values:\n  first\n    Which is: α\n  second\n    Which is: β";
    assert_eq!(
        parse(text, false).unwrap(),
        AssertionEvidence {
            format: "gtest/EXPECT_EQ".into(),
            expression: Some("left == right".into()),
            actual: None,
            expected: None,
            left: Some("α".into()),
            right: Some("β".into()),
            operands: vec!["α".into(), "β".into()],
            excerpt: text.into(),
        }
    );
    let text = "Value of: ready()\nnoise\n  Expected: true\n  Actual: false";
    assert_eq!(
        parse(text, false).unwrap(),
        AssertionEvidence {
            format: "gtest".into(),
            expression: Some("ready()".into()),
            actual: Some("false".into()),
            expected: Some("true".into()),
            left: Some("false".into()),
            right: Some("true".into()),
            operands: vec!["false".into(), "true".into()],
            excerpt: text.into(),
        }
    );
    for text in [
        "",
        "Expected equality of these values:\nWhich is: 1",
        "Value of: x\nActual: false",
        "Value of: x\nExpected: true",
    ] {
        assert_eq!(parse(text, false), None);
    }
}

#[test]
fn coverage_catch_expansions() {
    for prefix in ["CHECK", "REQUIRE"] {
        for (expansion, left, right) in [
            ("1 == 2", Some("1"), Some("2")),
            ("α != β", Some("α"), Some("β")),
            ("false", None, None),
        ] {
            let text = format!("{prefix}( ready() )\nwith expansion:\n{expansion}");
            assert_eq!(
                parse(&text, true).unwrap(),
                AssertionEvidence {
                    format: "catch2".into(),
                    expression: Some("ready()".into()),
                    actual: None,
                    expected: None,
                    left: left.map(str::to_string),
                    right: right.map(str::to_string),
                    operands: match (left, right) {
                        (Some(l), Some(r)) => vec![l.into(), r.into()],
                        _ => vec![expansion.into()],
                    },
                    excerpt: text,
                }
            );
        }
    }
    for text in ["", "CHECK(x", "CHECK(x)", "REQUIRE(x)\nother\nfalse"] {
        assert_eq!(parse(text, true), None);
    }
}
