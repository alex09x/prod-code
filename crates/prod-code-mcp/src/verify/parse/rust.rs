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

/// Parses one `cargo --message-format=json` line into a diagnostic (None for non-diagnostics
/// and for the trailing "N warnings emitted" summaries).
pub fn parse_cargo_json_line(line: &str) -> Option<Diagnostic> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("reason")?.as_str()? != "compiler-message" {
        return None;
    }
    let message = value.get("message")?;
    let level = message.get("level")?.as_str()?.to_string();
    if level != "error" && level != "warning" {
        return None;
    }
    let spans = message.get("spans").and_then(|s| s.as_array());
    let primary = spans.and_then(|spans| {
        spans
            .iter()
            .find(|s| {
                s.get("is_primary")
                    .and_then(|p| p.as_bool())
                    .unwrap_or(false)
            })
            .or_else(|| spans.first())
    });
    // No span: "aborting due to N previous errors", "N warnings emitted".
    let primary = primary?;
    Some(Diagnostic {
        level,
        code: message
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|c| c.as_str())
            .map(str::to_string),
        message: message.get("message")?.as_str()?.to_string(),
        file: primary
            .get("file_name")
            .and_then(|f| f.as_str())
            .map(str::to_string),
        line: primary.get("line_start").and_then(|l| l.as_u64()),
        column: primary.get("column_start").and_then(|c| c.as_u64()),
    })
}

/// Parses rustc's human-readable output (`error[E0425]: ...` followed by `--> file:line:col`).
pub fn parse_rustc_text(text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut pending: Option<Diagnostic> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        let header = line
            .strip_prefix("error")
            .map(|rest| ("error", rest))
            .or_else(|| line.strip_prefix("warning").map(|rest| ("warning", rest)));
        if let Some((level, rest)) = header
            && let Some((code_part, msg)) = rest.split_once(": ")
            && (code_part.is_empty() || (code_part.starts_with('[') && code_part.ends_with(']')))
        {
            if let Some(d) = pending.take() {
                out.push(d);
            }
            if msg.starts_with("aborting due to")
                || msg.contains("warning(s) emitted")
                || msg.contains("warnings emitted")
                || msg.starts_with("could not compile")
                || msg.starts_with("build failed")
            {
                continue;
            }
            pending = Some(Diagnostic {
                level: level.to_string(),
                code: (!code_part.is_empty())
                    .then(|| code_part.trim_matches(['[', ']']).to_string()),
                message: msg.to_string(),
                file: None,
                line: None,
                column: None,
            });
            continue;
        }
        if let Some(d) = pending.as_mut()
            && d.file.is_none()
            && let Some(loc) = line.trim_start().strip_prefix("--> ")
        {
            let mut parts = loc.rsplitn(3, ':');
            let col = parts.next().and_then(|c| c.parse().ok());
            let ln = parts.next().and_then(|l| l.parse().ok());
            let file = parts.next().map(str::to_string);
            if let Some(file) = file {
                d.file = Some(file);
                d.line = ln;
                d.column = col;
            }
        }
    }
    if let Some(d) = pending {
        out.push(d);
    }
    out
}

/// Parses `cargo test` (libtest) output: per-test results and failure output blocks.
pub fn parse_cargo_test_text(text: &str) -> (u64, u64, Vec<TestFailure>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut failures = Vec::new();
    let mut current: Option<TestFailure> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("test result: ")
            && let Some((_, counts)) = rest.split_once(". ")
        {
            for part in counts.split("; ") {
                let mut it = part.split_whitespace();
                if let (Some(n), Some(what)) = (it.next(), it.next())
                    && let Ok(n) = n.parse::<u64>()
                {
                    match what {
                        "passed" => passed += n,
                        "failed" => failed += n,
                        _ => {}
                    }
                }
            }
            continue;
        }
        if let Some(name) = line
            .strip_prefix("---- ")
            .and_then(|r| r.strip_suffix(" stdout ----"))
        {
            if let Some(f) = current.take() {
                failures.push(f);
            }
            current = Some(TestFailure {
                name: name.to_string(),
                output: String::new(),
            });
            continue;
        }
        if line == "failures:" || line.starts_with("test result:") {
            if let Some(f) = current.take() {
                failures.push(f);
            }
            continue;
        }
        if let Some(f) = current.as_mut() {
            f.output.push_str(line);
            f.output.push('\n');
        }
    }
    if let Some(f) = current {
        failures.push(f);
    }
    (passed, failed, failures)
}
