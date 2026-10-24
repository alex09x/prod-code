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

fn parse(text: &str, pytest: bool) -> Option<AssertionEvidence> {
    let raw: Vec<_> = text.lines().collect();
    let stripped: Vec<_> = raw.iter().map(|s| s.to_string()).collect();
    if pytest {
        parse_pytest_assertion(&raw, &stripped).flatten()
    } else {
        parse_python_unittest_assertion(&raw, &stripped).flatten()
    }
}

#[test]
fn coverage_pytest_operators_and_headers() {
    for (expr, suffix, left, right) in [
        ("α == β", "eq", "α", "β"),
        ("1 != 2", "ne", "1", "2"),
        ("x in items", "in", "x", "items"),
        ("3 > 4", "gt", "3", "4"),
        ("3 < 2", "lt", "3", "2"),
    ] {
        for header in [
            "E   assert ",
            "E\tAssertionError: assert ",
            "E AssertionError:   assert ",
        ] {
            let text = format!("noise\n{header}{expr}\nE   detail\nstop\nE ignored");
            let got = parse(&text, true).unwrap();
            assert_eq!(
                got,
                AssertionEvidence {
                    format: format!("pytest/assert_{suffix}"),
                    expression: Some(format!("assert {expr}")),
                    actual: None,
                    expected: None,
                    left: Some(left.into()),
                    right: Some(right.into()),
                    operands: vec![left.into(), right.into()],
                    excerpt: format!("{header}{expr}\nE   detail"),
                }
            );
        }
    }
    let got = parse("E assert ready()", true).unwrap();
    assert_eq!(got.format, "pytest/assert");
    assert_eq!(got.expression.as_deref(), Some("assert ready()"));
    assert_eq!(got.operands, ["ready()"]);
    assert_eq!((got.left, got.right), (None, None));
    for text in [
        "",
        "Error",
        "Eassert x",
        "E AssertionError: message",
        "E assert ",
    ] {
        assert_eq!(parse(text, true), None, "{text:?}");
    }
}

#[test]
fn coverage_unittest_variants_and_excerpt_boundaries() {
    for (text, format, expr, left, right, named) in [
        (
            "1 != 2",
            "assertEqual",
            "1 == 2",
            Some("1"),
            Some("2"),
            false,
        ),
        (
            "x == y",
            "assertNotEqual",
            "x != y",
            Some("x"),
            Some("y"),
            false,
        ),
        (
            "False is not true",
            "assertTrue",
            "assertTrue",
            Some("False"),
            Some("True"),
            true,
        ),
        (
            "True is not false",
            "assertFalse",
            "assertFalse",
            Some("True"),
            Some("False"),
            true,
        ),
        (
            "custom failure",
            "AssertionError",
            "custom failure",
            None,
            None,
            false,
        ),
    ] {
        for boundary in ["", "Traceback here", "FAIL: case"] {
            let input = format!(
                "noise\nAssertionError: {text}\n- old\n+ new\n? mark\ndetail\n{boundary}\nignored"
            );
            assert_eq!(
                parse(&input, false).unwrap(),
                AssertionEvidence {
                    format: format!("unittest/{format}"),
                    expression: Some(expr.into()),
                    actual: if named {
                        left.map(str::to_string)
                    } else {
                        None
                    },
                    expected: if named {
                        right.map(str::to_string)
                    } else {
                        None
                    },
                    left: left.map(str::to_string),
                    right: right.map(str::to_string),
                    operands: match (left, right) {
                        (Some(l), Some(r)) => vec![l.into(), r.into()],
                        _ => vec![text.into()],
                    },
                    excerpt: format!("AssertionError: {text}\n- old\n+ new\n? mark\ndetail"),
                }
            );
        }
    }
    assert_eq!(parse("AssertionError: assert a == b", false), None);
    assert_eq!(parse("no assertion", false), None);
}
