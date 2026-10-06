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

/// Parses `basedpyright --outputjson`: `generalDiagnostics[]` with file, range and severity.
pub fn parse_pyright_json(text: &str) -> Vec<Diagnostic> {
    let Some(start) = text.find('{') else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text[start..]) else {
        return Vec::new();
    };
    value
        .get("generalDiagnostics")
        .and_then(|d| d.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|d| {
                    let level = d.get("severity")?.as_str()?;
                    if level != "error" && level != "warning" {
                        return None;
                    }
                    Some(Diagnostic {
                        level: level.to_string(),
                        code: d.get("rule").and_then(|r| r.as_str()).map(str::to_string),
                        message: d.get("message")?.as_str()?.to_string(),
                        file: d.get("file").and_then(|f| f.as_str()).map(str::to_string),
                        line: d
                            .pointer("/range/start/line")
                            .and_then(|l| l.as_u64())
                            .map(|l| l + 1),
                        column: d
                            .pointer("/range/start/character")
                            .and_then(|c| c.as_u64())
                            .map(|c| c + 1),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parses pytest `-q -rf` output: the `FAILED path::test - message` summary lines and the
/// final `N passed, M failed` line.
pub fn parse_pytest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("FAILED ") {
            let (name, msg) = rest.split_once(" - ").unwrap_or((rest, ""));
            failures.push(TestFailure {
                name: name.trim().to_string(),
                output: msg.trim().to_string(),
            });
        }
        if line.contains(" passed") || line.contains(" failed") {
            for part in line.trim_matches(|c| c == '=' || c == ' ').split(", ") {
                let mut it = part.split_whitespace();
                if let (Some(n), Some(what)) = (it.next(), it.next())
                    && let Ok(n) = n.parse::<u64>()
                {
                    match what.trim_end_matches(|c: char| !c.is_alphabetic()) {
                        "passed" => passed = n,
                        "failed" => failed = n,
                        _ => {}
                    }
                }
            }
        }
    }
    (passed, failed, failures)
}

/// What a script prod-code runs says about itself on stderr as `prod-code: …`, such as the Go
/// lint script falling back to go vet, as notes at the head of the report.
pub fn script_notes(stderr: &str) -> Vec<Diagnostic> {
    stderr
        .lines()
        .filter_map(|line| line.strip_prefix("prod-code: "))
        .map(|message| Diagnostic {
            level: "note".to_string(),
            code: None,
            message: message.trim().to_string(),
            file: None,
            line: None,
            column: None,
        })
        .collect()
}

/// Parses `python -m unittest -v` output: `test_x (mod.Class.test_x) ... ok|FAIL|ERROR`,
/// `Ran N tests`, and the `FAIL:` / `ERROR:` blocks with their tracebacks.
pub fn parse_unittest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for raw in text.lines() {
        let trimmed = raw.trim_end();
        if trimmed.ends_with("... ok") {
            passed += 1;
            continue;
        }
        if trimmed.ends_with("... FAIL") || trimmed.ends_with("... ERROR") {
            failed += 1;
            continue;
        }
        if let Some(name) = trimmed
            .strip_prefix("FAIL: ")
            .or_else(|| trimmed.strip_prefix("ERROR: "))
        {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            current = Some((name.trim().to_string(), Vec::new()));
            continue;
        }
        if trimmed.starts_with("Ran ") || trimmed.starts_with("======") {
            if let Some((n, lines)) = current.take() {
                failures.push(TestFailure {
                    name: n,
                    output: lines.join("\n"),
                });
            }
            continue;
        }
        if trimmed.starts_with("------") {
            continue;
        }
        if let Some((_, lines)) = current.as_mut()
            && lines.len() < 40
            && !trimmed.trim().is_empty()
        {
            lines.push(trimmed.to_string());
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
