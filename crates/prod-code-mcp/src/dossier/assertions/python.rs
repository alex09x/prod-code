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

/// Python pytest assertion parser.
/// Format: `E   AssertionError: assert left == right` or `E   assert left == right`
pub(crate) fn parse_pytest_assertion(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let (header, rest) = stripped.iter().enumerate().find_map(|(i, line)| {
        let trimmed = line.trim();
        let after_e = trimmed.strip_prefix('E')?;
        if !after_e.starts_with(' ') && !after_e.starts_with('\t') {
            return None;
        }
        let after_e = after_e.trim();
        let content = if let Some(after) = after_e.strip_prefix("AssertionError: assert ") {
            after
        } else if let Some(after) = after_e.strip_prefix("AssertionError:") {
            let after = after.trim();
            if !after.starts_with("assert ") {
                return None;
            }
            after
        } else if let Some(after) = after_e.strip_prefix("assert ") {
            after
        } else {
            return None;
        };
        let content = content.trim();
        let content = content.strip_prefix("assert ").unwrap_or(content).trim();
        if content.is_empty() {
            return None;
        }
        Some((i, content))
    })?;

    let mut end = header;
    for i in header + 1..stripped.len() {
        let t = stripped[i].trim();
        if t.starts_with('E') {
            end = i;
        } else {
            break;
        }
    }

    let (left, right, format) = if let Some((l, r)) = rest.split_once(" == ") {
        (
            Some(l.trim().to_string()),
            Some(r.trim().to_string()),
            "pytest/assert_eq",
        )
    } else if let Some((l, r)) = rest.split_once(" != ") {
        (
            Some(l.trim().to_string()),
            Some(r.trim().to_string()),
            "pytest/assert_ne",
        )
    } else if let Some((l, r)) = rest.split_once(" in ") {
        (
            Some(l.trim().to_string()),
            Some(r.trim().to_string()),
            "pytest/assert_in",
        )
    } else if let Some((l, r)) = rest.split_once(" > ") {
        (
            Some(l.trim().to_string()),
            Some(r.trim().to_string()),
            "pytest/assert_gt",
        )
    } else if let Some((l, r)) = rest.split_once(" < ") {
        (
            Some(l.trim().to_string()),
            Some(r.trim().to_string()),
            "pytest/assert_lt",
        )
    } else {
        (None, None, "pytest/assert")
    };

    let operands = match (&left, &right) {
        (Some(l), Some(r)) => vec![l.clone(), r.clone()],
        (Some(l), None) => vec![l.clone()],
        _ => vec![rest.to_string()],
    };

    Some(Some(AssertionEvidence {
        format: format.to_string(),
        expression: Some(format!("assert {rest}")),
        actual: None,
        expected: None,
        left,
        right,
        operands,
        excerpt: raw_excerpt(raw, header, end),
    }))
}

/// Python standard unittest assertion parser.
/// Format: `AssertionError: 1 != 2` (assertEqual) or `AssertionError: False is not true`
pub(crate) fn parse_python_unittest_assertion(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let (header, rest) = stripped.iter().enumerate().find_map(|(i, line)| {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix("AssertionError: ")?;
        if rest.starts_with("assert ") {
            return None;
        }
        Some((i, rest.trim()))
    })?;

    let mut end = header;
    for i in header + 1..stripped.len() {
        let t = stripped[i].trim();
        if t.starts_with('-')
            || t.starts_with('+')
            || t.starts_with('?')
            || (!t.is_empty() && !t.starts_with("Traceback") && !t.starts_with("FAIL:"))
        {
            end = i;
        } else {
            break;
        }
    }

    if let Some((left, right)) = rest.split_once(" != ") {
        Some(Some(AssertionEvidence {
            format: "unittest/assertEqual".to_string(),
            expression: Some(format!("{} == {}", left.trim(), right.trim())),
            actual: None,
            expected: None,
            left: Some(left.trim().to_string()),
            right: Some(right.trim().to_string()),
            operands: vec![left.trim().to_string(), right.trim().to_string()],
            excerpt: raw_excerpt(raw, header, end),
        }))
    } else if let Some((left, right)) = rest.split_once(" == ") {
        Some(Some(AssertionEvidence {
            format: "unittest/assertNotEqual".to_string(),
            expression: Some(format!("{} != {}", left.trim(), right.trim())),
            actual: None,
            expected: None,
            left: Some(left.trim().to_string()),
            right: Some(right.trim().to_string()),
            operands: vec![left.trim().to_string(), right.trim().to_string()],
            excerpt: raw_excerpt(raw, header, end),
        }))
    } else if rest == "False is not true" {
        Some(Some(AssertionEvidence {
            format: "unittest/assertTrue".to_string(),
            expression: Some("assertTrue".to_string()),
            actual: Some("False".to_string()),
            expected: Some("True".to_string()),
            left: Some("False".to_string()),
            right: Some("True".to_string()),
            operands: vec!["False".to_string(), "True".to_string()],
            excerpt: raw_excerpt(raw, header, end),
        }))
    } else if rest == "True is not false" {
        Some(Some(AssertionEvidence {
            format: "unittest/assertFalse".to_string(),
            expression: Some("assertFalse".to_string()),
            actual: Some("True".to_string()),
            expected: Some("False".to_string()),
            left: Some("True".to_string()),
            right: Some("False".to_string()),
            operands: vec!["True".to_string(), "False".to_string()],
            excerpt: raw_excerpt(raw, header, end),
        }))
    } else {
        Some(Some(AssertionEvidence {
            format: "unittest/AssertionError".to_string(),
            expression: Some(rest.to_string()),
            actual: None,
            expected: None,
            left: None,
            right: None,
            operands: vec![rest.to_string()],
            excerpt: raw_excerpt(raw, header, end),
        }))
    }
}
