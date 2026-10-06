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

/// Go testify assertion parser.
/// Format: `Error: Not equal:\n expected: 1\n actual : 2`
pub(crate) fn parse_go_testify_assertion(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let header = stripped.iter().position(|l| {
        let t = l.trim();
        (t.contains("Error:") && (t.contains("Not equal") || t.contains("Equal values expected")))
            || (t.contains("expected:") && stripped.iter().any(|s| s.contains("actual")))
    })?;

    let mut expected = None;
    let mut actual = None;
    let mut end = header;
    for i in header..stripped.len().min(header + 12) {
        let trimmed = stripped[i].trim();
        if let Some((_, val)) = trimmed.split_once("expected:") {
            expected = Some(val.trim().to_string());
            end = end.max(i);
        } else if let Some((_, val)) = trimmed.split_once("actual") {
            let val = val.trim_start();
            if let Some(val) = val.strip_prefix(':') {
                actual = Some(val.trim().to_string());
                end = end.max(i);
            }
        }
    }

    let (exp, act) = (expected?, actual?);
    Some(Some(AssertionEvidence {
        format: "testify/assert".to_string(),
        expression: Some("expected == actual".to_string()),
        actual: Some(act.clone()),
        expected: Some(exp.clone()),
        left: Some(act.clone()),
        right: Some(exp.clone()),
        operands: vec![act, exp],
        excerpt: raw_excerpt(raw, header, end),
    }))
}

/// Standard Go test assertion parser.
/// Format: `got 1, want 2` or `got: 1, expected: 2`
pub(crate) fn parse_go_got_want_assertion(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let (header, got, want) = stripped.iter().enumerate().find_map(|(i, line)| {
        let t = line.trim();
        if !t.contains("got") || (!t.contains("want") && !t.contains("expected")) {
            return None;
        }
        let got_idx = t.find("got")?;
        let want_idx = t.find("want").or_else(|| t.find("expected"))?;
        if got_idx >= want_idx {
            return None;
        }

        let got_part = &t[got_idx..want_idx];
        let want_part = &t[want_idx..];

        let got_val = got_part
            .strip_prefix("got:")
            .or_else(|| got_part.strip_prefix("got"))?
            .trim()
            .trim_end_matches([',', ';', ':'])
            .trim();

        let want_val = want_part
            .strip_prefix("want:")
            .or_else(|| want_part.strip_prefix("want"))
            .or_else(|| want_part.strip_prefix("expected:"))
            .or_else(|| want_part.strip_prefix("expected"))?
            .trim()
            .trim_end_matches([',', ';', '.'])
            .trim();

        if got_val.is_empty() || want_val.is_empty() {
            return None;
        }

        Some((i, got_val.to_string(), want_val.to_string()))
    })?;

    Some(Some(AssertionEvidence {
        format: "go/got_want".to_string(),
        expression: Some("got == want".to_string()),
        actual: Some(got.clone()),
        expected: Some(want.clone()),
        left: Some(got.clone()),
        right: Some(want.clone()),
        operands: vec![got, want],
        excerpt: raw[header].to_string(),
    }))
}
