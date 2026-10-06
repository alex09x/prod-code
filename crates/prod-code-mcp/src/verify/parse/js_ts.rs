/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::{Diagnostic, TestFailure};

/// Parses `tsc --pretty false` lines: `src/a.ts(12,5): error TS2322: message`.
pub fn parse_tsc_text(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| {
            let (loc, rest) = line.split_once("): ")?;
            let (file, pos) = loc.rsplit_once('(')?;
            let (ln, col) = pos.split_once(',')?;
            let (level, rest) = rest.split_once(' ')?;
            let (code, message) = rest.split_once(": ")?;
            Some(Diagnostic {
                level: level.to_string(),
                code: Some(code.to_string()),
                message: message.to_string(),
                file: Some(file.trim().to_string()),
                line: ln.parse().ok(),
                column: col.parse().ok(),
            })
        })
        .collect()
}

pub fn parse_jest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(rest) = line.trim().strip_prefix("Tests:") {
            for part in rest.split(',') {
                let part = part.trim();
                let mut it = part.split_whitespace();
                let n: u64 = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                match it.next() {
                    Some("passed") => passed = n,
                    Some("failed") => failed = n,
                    _ => {}
                }
            }
            continue;
        }
        if let Some(name) = line.trim().strip_prefix("● ") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            if !name.starts_with("Test suite failed") {
                current = Some((name.trim().to_string(), Vec::new()));
            }
            continue;
        }
        if let Some((_, lines)) = current.as_mut()
            && lines.len() < 40
        {
            lines.push(line.to_string());
        }
    }
    if let Some((n, lines)) = current.take() {
        failures.push(TestFailure {
            name: n,
            output: lines.join("\n"),
        });
    }
    failures.retain(|f| !f.name.is_empty());
    (passed, failed, failures)
}

/// Parses vitest output: ` Tests  1 failed | 2 passed (3)` and `FAIL  file > name` / ` × name`.
pub fn parse_vitest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Tests ") {
            for part in rest.split('|') {
                let mut it = part.split_whitespace();
                let n: u64 = it.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                match it.next() {
                    Some("passed") => passed = n,
                    Some("failed") => failed = n,
                    _ => {}
                }
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("FAIL ") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            current = Some((rest.trim().to_string(), Vec::new()));
            continue;
        }
        if trimmed.starts_with("Test Files") || trimmed.starts_with("Start at") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            continue;
        }
        if let Some((_, lines)) = current.as_mut()
            && lines.len() < 40
            && !trimmed.is_empty()
        {
            lines.push(line.to_string());
        }
    }
    if let Some((n, lines)) = current.take() {
        failures.push(TestFailure {
            name: n,
            output: lines.join("\n"),
        });
    }
    (passed, failed, failures)
}

/// Parses `bun test` output: ` 2 pass`, ` 1 fail` and `(fail) name` lines.
pub fn parse_bun_test_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for raw in text.lines() {
        let trimmed = raw.trim();
        let mut it = trimmed.split_whitespace();
        if let (Some(n), Some(word), None) = (it.next(), it.next(), it.next())
            && let Ok(n) = n.parse::<u64>()
        {
            match word {
                "pass" => {
                    passed = n;
                    continue;
                }
                "fail" => {
                    failed = n;
                    continue;
                }
                _ => {}
            }
        }
        if let Some(name) = trimmed.strip_prefix("(fail) ") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            current = Some((
                name.split(" [").next().unwrap_or(name).to_string(),
                Vec::new(),
            ));
            continue;
        }
        if trimmed.starts_with("(pass) ") || trimmed.starts_with("Ran ") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            continue;
        }
        if let Some((_, lines)) = current.as_mut()
            && lines.len() < 40
            && !trimmed.is_empty()
        {
            lines.push(raw.trim_end().to_string());
        }
    }
    if let Some((n, lines)) = current.take() {
        failures.push(TestFailure {
            name: n,
            output: lines.join("\n"),
        });
    }
    (passed, failed, failures)
}
