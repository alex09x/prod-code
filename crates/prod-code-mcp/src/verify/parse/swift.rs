/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::verify::parse::common::parse_colon_diagnostics;
use crate::verify::types::{Diagnostic, TestFailure};

/// Parses Swift compiler and SwiftPM diagnostics from `swift build` and `swift test` output:
/// standard `file:line:col: (error|warning): message` lines, plus SwiftPM dependency resolution
/// errors (`GitShellError`, SSH authentication failures, manifest errors).
pub fn parse_swift_text(text: &str) -> Vec<Diagnostic> {
    let mut out = parse_colon_diagnostics(text);

    let mut fetching_urls: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut last_fetch_url: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(url) = trimmed
            .strip_prefix("Fetching ")
            .or_else(|| trimmed.strip_prefix("Cloning "))
        {
            let url = url.trim();
            last_fetch_url = Some(url.to_string());
            if let Some(name) = url.rsplit('/').next().map(|s| s.trim_end_matches(".git"))
                && !name.is_empty()
            {
                fetching_urls.insert(name.to_string(), url.to_string());
            }
            continue;
        }

        if trimmed.contains("GitShellError") {
            let pkg = if let Some(part) = trimmed
                .split("':")
                .next()
                .and_then(|p| p.rsplit('\'').next())
            {
                let p = part.trim();
                (!p.is_empty()).then_some(p)
            } else {
                None
            };

            let associated_url = pkg
                .and_then(|p| fetching_urls.get(p))
                .cloned()
                .or_else(|| last_fetch_url.clone());

            let is_ssh = associated_url
                .as_deref()
                .map(|u| u.starts_with("git@") || u.starts_with("ssh://"))
                .unwrap_or(false);

            let exit_code = if let Some(code_part) = trimmed.split("exit: terminated(code: ").nth(1)
            {
                code_part
                    .split(')')
                    .next()
                    .and_then(|c| c.parse::<i32>().ok())
            } else {
                None
            };

            let error_output = if let Some(out_part) = trimmed.split("output:").nth(1) {
                let cleaned = out_part.trim_end_matches([')', '>', ' ']);
                (!cleaned.is_empty()).then(|| cleaned.to_string())
            } else {
                None
            };

            let pkg_label = pkg
                .map(|p| format!("dependency '{p}'"))
                .unwrap_or_else(|| "dependency".to_string());
            let url_label = associated_url
                .as_ref()
                .map(|u| format!(" from {u}"))
                .unwrap_or_default();

            let reason = if let Some(out) = error_output {
                format!("Git error: {out}")
            } else if is_ssh || exit_code == Some(128) {
                if is_ssh {
                    "Git SSH authentication failed (exit code 128); verify SSH key access on the build node or repository permissions".to_string()
                } else {
                    "Git clone failed (exit code 128); verify repository credentials and access permissions on the build node".to_string()
                }
            } else if let Some(code) = exit_code {
                format!("Git command failed with exit code {code}")
            } else {
                "Git clone failed with GitShellError".to_string()
            };

            out.push(Diagnostic {
                level: "error".to_string(),
                code: Some("git-fetch".to_string()),
                message: format!("failed to fetch {pkg_label}{url_label}: {reason}"),
                file: Some("Package.swift".to_string()),
                line: None,
                column: None,
            });
            continue;
        }

        if let Some(msg) = trimmed.strip_prefix("error: ") {
            let msg = msg.trim();
            if !msg.is_empty()
                && !msg.starts_with("fatalError")
                && !msg.starts_with("build failed")
                && !msg.contains("error(s) generated")
            {
                out.push(Diagnostic {
                    level: "error".to_string(),
                    code: None,
                    message: msg.to_string(),
                    file: Some("Package.swift".to_string()),
                    line: None,
                    column: None,
                });
            }
        } else if let Some(msg) = trimmed.strip_prefix("warning: ") {
            let msg = msg.trim();
            if !msg.is_empty() && !msg.contains("warning(s) generated") {
                out.push(Diagnostic {
                    level: "warning".to_string(),
                    code: None,
                    message: msg.to_string(),
                    file: Some("Package.swift".to_string()),
                    line: None,
                    column: None,
                });
            }
        }
    }

    out.dedup();
    out
}

/// Parses XCTest (`swift test`) output on macOS and Linux: `Test Case '-[Suite test]' passed
/// (0.001 seconds)` / `Test Case 'Suite.test' failed`, assertion lines `file:line: error:
/// -[Suite test] : message`, plus swift-testing `✔ Test "name" passed` / `✘ Test "name" failed`.
pub fn parse_xctest_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures: Vec<TestFailure> = Vec::new();
    let mut assertions: Vec<(String, String)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("Test Case '") {
            let Some((name, status)) = rest.split_once("' ") else {
                continue;
            };
            let name = name
                .trim_start_matches("-[")
                .trim_end_matches(']')
                .replace(' ', ".");
            if status.starts_with("passed") {
                passed += 1;
            } else if status.starts_with("failed") {
                failed += 1;
                let output = assertions
                    .iter()
                    .filter(|(test, _)| *test == name)
                    .map(|(_, msg)| msg.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                failures.push(TestFailure { name, output });
            }
            continue;
        }
        if let Some(pos) = line.find(": error: -[") {
            let location = &line[..pos];
            let rest = &line[pos + ": error: -[".len()..];
            if let Some((test, message)) = rest.split_once("] : ") {
                assertions.push((
                    test.replace(' ', "."),
                    format!("{location}: {}", message.trim()),
                ));
            }
            continue;
        }
        // swift-testing: `✔ Test "adds"() passed after 0.001 seconds.`
        if let Some(rest) = line
            .strip_prefix("✔ Test ")
            .or_else(|| line.strip_prefix("✘ Test "))
        {
            if rest.starts_with("run ") {
                continue;
            }
            let name = rest
                .split(" passed")
                .next()
                .and_then(|n| n.split(" failed").next())
                .unwrap_or(rest)
                .trim_matches('"')
                .to_string();
            if line.starts_with('✔') {
                passed += 1;
            } else {
                failed += 1;
                failures.push(TestFailure {
                    name,
                    output: line.to_string(),
                });
            }
        }
    }
    (passed, failed, failures)
}
