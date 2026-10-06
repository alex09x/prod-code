/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{RemoteExecDiagnostic, RemoteExecSpan, RemoteExecStream, RemoteExecTestEvent};

/// Parse a line from `cargo --message-format=json` (or standard Cargo/libtest output) into a diagnostic or test event if applicable.
pub fn parse_cargo_json_event(line: &str) -> Option<RemoteExecStream> {
    let trimmed = line.trim();

    // 1. Try parsing JSON format (compiler messages or unstable/custom json test records)
    if trimmed.starts_with('{')
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        if let Some(reason) = value.get("reason").and_then(|r| r.as_str())
            && reason == "compiler-message"
            && let Some(msg) = value.get("message")
        {
            let level = msg
                .get("level")
                .and_then(|l| l.as_str())
                .unwrap_or("error")
                .to_string();
            let message_text = msg
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            let code = msg
                .get("code")
                .and_then(|c| c.get("code"))
                .and_then(|c| c.as_str())
                .map(str::to_string);
            let rendered = msg
                .get("rendered")
                .and_then(|r| r.as_str())
                .map(str::to_string);

            let mut spans = Vec::new();
            if let Some(spans_arr) = msg.get("spans").and_then(|s| s.as_array()) {
                for s in spans_arr {
                    if let Some(file) = s.get("file_name").and_then(|f| f.as_str()) {
                        let line_start =
                            s.get("line_start").and_then(|l| l.as_u64()).unwrap_or(0) as u32;
                        let line_end = s.get("line_end").and_then(|l| l.as_u64()).map(|l| l as u32);
                        let col_start =
                            s.get("column_start").and_then(|c| c.as_u64()).unwrap_or(0) as u32;
                        let col_end = s
                            .get("column_end")
                            .and_then(|c| c.as_u64())
                            .map(|c| c as u32);
                        let is_primary = s
                            .get("is_primary")
                            .and_then(|p| p.as_bool())
                            .unwrap_or(false);
                        let label = s
                            .get("label")
                            .and_then(|lbl| lbl.as_str())
                            .map(str::to_string);
                        spans.push(RemoteExecSpan {
                            file: file.to_string(),
                            line_start,
                            line_end,
                            col_start,
                            col_end,
                            is_primary,
                            label,
                        });
                    }
                }
            }

            return Some(RemoteExecStream::Diagnostic(RemoteExecDiagnostic {
                level,
                code,
                message: message_text,
                spans,
                rendered,
                suggestion: None,
            }));
        }

        if let Some(t) = value.get("type").and_then(|t| t.as_str())
            && t == "test"
        {
            let event = value.get("event").and_then(|e| e.as_str()).unwrap_or("");
            let name = value
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let duration_ms = value
                .get("exec_time")
                .and_then(|t| t.as_f64())
                .map(|s| (s * 1000.0) as u64);
            match event {
                "started" => {
                    return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Started {
                        name,
                    }));
                }
                "ok" => {
                    return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed {
                        name,
                        duration_ms,
                    }));
                }
                "failed" => {
                    let output = value
                        .get("stdout")
                        .and_then(|o| o.as_str())
                        .map(str::to_string);
                    return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed {
                        name,
                        duration_ms,
                        message: None,
                        assertion_diff: None,
                        backtrace: None,
                        output,
                    }));
                }
                "ignored" => {
                    return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped {
                        name,
                        reason: None,
                    }));
                }
                "bench" => {
                    let median = value.get("median").and_then(|m| m.as_f64()).unwrap_or(0.0);
                    return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Bench {
                        name,
                        estimate: format!("{median:.2} ns/iter"),
                        range: None,
                    }));
                }
                _ => {}
            }
        }
    }

    // 2. Parse standard Cargo/libtest text output lines (emitted by the test runner during `cargo test`)
    // Format: `test <name> ... ok` / `test <name> ... FAILED` / `test <name> ... ignored` / `test <name> ... bench: <est>`
    if let Some(rest) = trimmed.strip_prefix("test ")
        && !rest.starts_with("result:")
        && let Some((name, outcome_part)) = rest.rsplit_once(" ... ")
    {
        let test_name = name.trim().to_string();
        let outcome = outcome_part.trim();
        if outcome == "ok" || outcome.starts_with("ok ") {
            let duration_ms = outcome
                .find('(')
                .and_then(|open| {
                    outcome[open..]
                        .find('s')
                        .map(|close| &outcome[open + 1..open + close])
                })
                .and_then(|s_str| s_str.trim().parse::<f64>().ok())
                .map(|s| (s * 1000.0) as u64);
            return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed {
                name: test_name,
                duration_ms,
            }));
        } else if outcome == "FAILED" || outcome.starts_with("FAILED ") {
            return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed {
                name: test_name,
                duration_ms: None,
                message: None,
                assertion_diff: None,
                backtrace: None,
                output: None,
            }));
        } else if outcome == "ignored" || outcome.starts_with("ignored ") {
            return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped {
                name: test_name,
                reason: None,
            }));
        } else if let Some(bench_str) = outcome.strip_prefix("bench:") {
            return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Bench {
                name: test_name,
                estimate: bench_str.trim().to_string(),
                range: None,
            }));
        }
    }

    None
}
