/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeMap, BTreeSet};

use super::super::types::{Diagnostic, TestFailure};

pub fn parse_go_text(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                return None;
            }
            let mut parts = line.splitn(4, ':');
            let file = parts.next()?;
            let ln = parts.next()?.trim().parse::<u64>().ok()?;
            let rest = parts.next()?;
            let (col, message) = match (rest.trim().parse::<u64>(), parts.next()) {
                (Ok(col), Some(msg)) => (Some(col), msg.trim().to_string()),
                _ => (None, rest.trim().to_string()),
            };
            if !file.ends_with(".go") {
                return None;
            }
            Some(Diagnostic {
                level: "error".to_string(),
                code: None,
                message,
                file: Some(file.to_string()),
                line: Some(ln),
                column: col,
            })
        })
        .collect()
}

/// The findings of golangci-lint's JSON output, which a project's config can select instead
/// of text (`{"Issues":[{"FromLinter":…,"Text":…,"Pos":{"Filename":…,"Line":…,"Column":…}}]}`,
/// one object per line or the whole stdout). Text that is not that report gives nothing, so
/// the text parser's findings stand (#1480).
pub fn parse_golangci_json(text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for chunk in std::iter::once(text).chain(text.lines()) {
        let Ok(report) = serde_json::from_str::<serde_json::Value>(chunk.trim()) else {
            continue;
        };
        let Some(issues) = report.get("Issues").and_then(|i| i.as_array()) else {
            continue;
        };
        for issue in issues {
            let pos = issue.get("Pos");
            let Some(file) = pos.and_then(|p| p.get("Filename")).and_then(|f| f.as_str()) else {
                continue;
            };
            let text = issue
                .get("Text")
                .and_then(|t| t.as_str())
                .unwrap_or_default();
            let message = match issue.get("FromLinter").and_then(|l| l.as_str()) {
                Some(linter) if !linter.is_empty() => format!("{text} ({linter})"),
                _ => text.to_string(),
            };
            out.push(Diagnostic {
                level: "error".to_string(),
                code: None,
                message,
                file: Some(file.to_string()),
                line: pos.and_then(|p| p.get("Line")).and_then(|l| l.as_u64()),
                column: pos
                    .and_then(|p| p.get("Column"))
                    .and_then(|c| c.as_u64())
                    .filter(|c| *c > 0),
            });
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

/// Parses `go test -json` events into pass/fail counts and per-test failure output.
pub fn parse_go_test_json(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut outputs: BTreeMap<String, String> = Default::default();
    let mut pkg_outputs: BTreeMap<String, String> = Default::default();
    let mut pkg_had_test_failures: BTreeSet<String> = Default::default();
    let mut failures = Vec::new();
    for line in text.lines() {
        let Ok(ev) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // Go 1.25 build-output/build-fail events use ImportPath where test events use Package.
        // Keep both under the same package key so a later package-level fail retains compiler text.
        let pkg = ev
            .get("Package")
            .and_then(|p| p.as_str())
            .or_else(|| ev.get("ImportPath").and_then(|p| p.as_str()))
            .unwrap_or("");
        if let Some(test) = ev.get("Test").and_then(|t| t.as_str()) {
            let key = format!("{pkg}.{test}");
            match ev.get("Action").and_then(|a| a.as_str()) {
                Some("output") => {
                    outputs
                        .entry(key)
                        .or_default()
                        .push_str(ev.get("Output").and_then(|o| o.as_str()).unwrap_or(""));
                }
                Some("pass") => passed += 1,
                Some("fail") => {
                    failed += 1;
                    pkg_had_test_failures.insert(pkg.to_string());
                    failures.push(TestFailure {
                        name: key.clone(),
                        output: outputs.remove(&key).unwrap_or_default(),
                    });
                }
                _ => {}
            }
        } else {
            match ev.get("Action").and_then(|a| a.as_str()) {
                Some("output" | "build-output") => {
                    pkg_outputs
                        .entry(pkg.to_string())
                        .or_default()
                        .push_str(ev.get("Output").and_then(|o| o.as_str()).unwrap_or(""));
                }
                Some("build-fail") => {
                    if let Some(output) = ev.get("Output").and_then(|o| o.as_str()) {
                        pkg_outputs
                            .entry(pkg.to_string())
                            .or_default()
                            .push_str(output);
                    }
                }
                Some("pass") => {
                    pkg_outputs.remove(pkg);
                }
                Some("fail") => {
                    if !pkg_had_test_failures.contains(pkg) {
                        failed += 1;
                        let name = if pkg.is_empty() {
                            "package".to_string()
                        } else {
                            pkg.to_string()
                        };
                        failures.push(TestFailure {
                            name,
                            output: pkg_outputs.remove(pkg).unwrap_or_default(),
                        });
                        pkg_had_test_failures.insert(pkg.to_string());
                    } else {
                        pkg_outputs.remove(pkg);
                    }
                }
                _ => {}
            }
        }
    }
    (passed, failed, failures)
}
