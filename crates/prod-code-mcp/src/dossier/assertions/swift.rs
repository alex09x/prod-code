/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::dossier::types::AssertionEvidence;

/// Swift XCTest assertion parser.
/// Format: `XCTAssertEqual failed: ("1") is not equal to ("2")`
pub(crate) fn parse_swift_xctest_assertion(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let (header, is_eq, left, right) = stripped.iter().enumerate().find_map(|(i, line)| {
        let t = line.trim();
        if let Some((_, rest)) = t.split_once("XCTAssertEqual failed: (\"") {
            let (l, r_rest) = rest.split_once("\") is not equal to (\"")?;
            let r = r_rest.split_once("\")")?.0;
            Some((i, true, l.to_string(), r.to_string()))
        } else if let Some((_, rest)) = t.split_once("XCTAssertNotEqual failed: (\"") {
            let (l, r_rest) = rest.split_once("\") is equal to (\"")?;
            let r = r_rest.split_once("\")")?.0;
            Some((i, false, l.to_string(), r.to_string()))
        } else {
            None
        }
    })?;

    Some(Some(AssertionEvidence {
        format: if is_eq {
            "XCTAssertEqual"
        } else {
            "XCTAssertNotEqual"
        }
        .to_string(),
        expression: Some(
            if is_eq {
                "left == right"
            } else {
                "left != right"
            }
            .to_string(),
        ),
        actual: None,
        expected: None,
        left: Some(left.clone()),
        right: Some(right.clone()),
        operands: vec![left, right],
        excerpt: raw[header].to_string(),
    }))
}

/// Swift Testing framework assertion parser (Swift 6+).
/// Format: `Expectation failed: (left → 1) == (right → 2)`
pub(crate) fn parse_swift_testing_assertion(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let (header, rest) = stripped.iter().enumerate().find_map(|(i, line)| {
        let t = line.trim();
        let rest = t.split_once("Expectation failed: ")?.1.trim();
        Some((i, rest))
    })?;

    fn clean_op(s: &str) -> String {
        let s = s.trim().trim_start_matches('(').trim_end_matches(')');
        if let Some((_, val)) = s.split_once('→') {
            val.trim().to_string()
        } else {
            s.to_string()
        }
    }

    let (left, right) = if let Some((l, r)) = rest.split_once(" == ") {
        (Some(clean_op(l)), Some(clean_op(r)))
    } else if let Some((l, r)) = rest.split_once(" != ") {
        (Some(clean_op(l)), Some(clean_op(r)))
    } else {
        (None, None)
    };

    let operands = match (&left, &right) {
        (Some(l), Some(r)) => vec![l.clone(), r.clone()],
        _ => vec![rest.to_string()],
    };

    Some(Some(AssertionEvidence {
        format: "swift-testing".to_string(),
        expression: Some(rest.to_string()),
        actual: None,
        expected: None,
        left,
        right,
        operands,
        excerpt: raw[header].to_string(),
    }))
}
