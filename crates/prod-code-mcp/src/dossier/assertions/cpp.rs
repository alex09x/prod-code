/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::util::raw_excerpt;
use crate::dossier::types::AssertionEvidence;

/// C++ GoogleTest (gtest) assertion parser.
pub(crate) fn parse_gtest_assertion(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    if let Some(header) = stripped
        .iter()
        .position(|l| l.trim() == "Expected equality of these values:")
    {
        let mut which = Vec::new();
        let mut end = header;
        for i in header + 1..stripped.len().min(header + 8) {
            let line = stripped[i].trim();
            if let Some((_, val)) = line.split_once("Which is:") {
                which.push(val.trim().to_string());
                end = i;
            }
        }
        if which.len() >= 2 {
            let left = which[0].clone();
            let right = which[1].clone();
            return Some(Some(AssertionEvidence {
                format: "gtest/EXPECT_EQ".to_string(),
                expression: Some("left == right".to_string()),
                actual: None,
                expected: None,
                left: Some(left.clone()),
                right: Some(right.clone()),
                operands: vec![left, right],
                excerpt: raw_excerpt(raw, header, end),
            }));
        }
    }

    if let Some((header, expr)) = stripped.iter().enumerate().find_map(|(i, l)| {
        let t = l.trim();
        let expr = t.strip_prefix("Value of: ")?;
        Some((i, expr.trim()))
    }) {
        let mut actual = None;
        let mut expected = None;
        let mut end = header;
        for i in header + 1..stripped.len().min(header + 6) {
            let line = stripped[i].trim();
            if let Some((_, val)) = line.split_once("Actual:") {
                actual = Some(val.trim().to_string());
                end = end.max(i);
            } else if let Some((_, val)) = line.split_once("Expected:") {
                expected = Some(val.trim().to_string());
                end = end.max(i);
            }
        }
        if let (Some(act), Some(exp)) = (actual, expected) {
            return Some(Some(AssertionEvidence {
                format: "gtest".to_string(),
                expression: Some(expr.to_string()),
                actual: Some(act.clone()),
                expected: Some(exp.clone()),
                left: Some(act.clone()),
                right: Some(exp.clone()),
                operands: vec![act, exp],
                excerpt: raw_excerpt(raw, header, end),
            }));
        }
    }

    None
}

/// C++ Catch2 assertion parser.
pub(crate) fn parse_catch2_assertion(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let (header, expr) = stripped.iter().enumerate().find_map(|(i, l)| {
        let t = l.trim();
        let expr = if let Some(e) = t
            .strip_prefix("CHECK(")
            .or_else(|| t.strip_prefix("REQUIRE("))
        {
            e.strip_suffix(')')?
        } else {
            return None;
        };
        Some((i, expr.trim()))
    })?;

    if header + 2 < stripped.len() && stripped[header + 1].trim().contains("with expansion:") {
        let expansion = stripped[header + 2].trim();
        let (left, right) = if let Some((l, r)) = expansion.split_once(" == ") {
            (Some(l.trim().to_string()), Some(r.trim().to_string()))
        } else if let Some((l, r)) = expansion.split_once(" != ") {
            (Some(l.trim().to_string()), Some(r.trim().to_string()))
        } else {
            (None, None)
        };
        let operands = match (&left, &right) {
            (Some(l), Some(r)) => vec![l.clone(), r.clone()],
            _ => vec![expansion.to_string()],
        };
        return Some(Some(AssertionEvidence {
            format: "catch2".to_string(),
            expression: Some(expr.to_string()),
            actual: None,
            expected: None,
            left,
            right,
            operands,
            excerpt: raw_excerpt(raw, header, header + 2),
        }));
    }

    None
}
