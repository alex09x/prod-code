/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::TestFailure;

pub fn parse_meson_test_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        let Some((idx, rest)) = trimmed.split_once(' ') else {
            continue;
        };
        if !idx.contains('/') || !idx.chars().all(|c| c.is_ascii_digit() || c == '/') {
            continue;
        }
        let words: Vec<&str> = rest.split_whitespace().collect();
        let Some(name) = words.first() else {
            continue;
        };
        if words.contains(&"OK") {
            passed += 1;
        } else if words
            .iter()
            .any(|w| matches!(*w, "FAIL" | "ERROR" | "TIMEOUT"))
        {
            failed += 1;
            failures.push(TestFailure {
                name: name.to_string(),
                output: rest.to_string(),
            });
        }
    }
    (passed, failed, failures)
}

/// Parses `ctest --output-on-failure` summaries: `1/3 Test #1: name ...... Passed` /
/// `***Failed`, with the failing test's output captured until the next test line.
pub fn parse_ctest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    let flush = |current: &mut Option<(String, Vec<String>)>, failures: &mut Vec<TestFailure>| {
        if let Some((name, lines)) = current.take() {
            failures.push(TestFailure {
                name,
                output: lines.join("\n"),
            });
        }
    };
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        let is_test_line = trimmed
            .split_once(' ')
            .is_some_and(|(n, rest)| n.contains('/') && rest.starts_with("Test #"));
        if is_test_line {
            flush(&mut current, &mut failures);
            let name = trimmed
                .split_once(": ")
                .map(|(_, r)| r.split(" .").next().unwrap_or(r).trim().to_string())
                .unwrap_or_default();
            if trimmed.ends_with("Passed") || trimmed.contains(" Passed ") {
                passed += 1;
            } else if trimmed.contains("Failed") || trimmed.contains("Timeout") {
                failed += 1;
                current = Some((name, Vec::new()));
            }
            continue;
        }
        if trimmed.starts_with("% tests passed") || trimmed.contains("% tests passed,") {
            flush(&mut current, &mut failures);
            continue;
        }
        if let Some((_, lines)) = current.as_mut() {
            lines.push(line.to_string());
        }
    }
    flush(&mut current, &mut failures);
    (passed, failed, failures)
}
