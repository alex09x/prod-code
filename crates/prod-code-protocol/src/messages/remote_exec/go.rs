/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{RemoteExecStream, RemoteExecTestEvent};

/// Parse a line from `go test` (either `-json` or standard human-readable format) into a test event if applicable.
pub fn parse_go_test_json_event(line: &str) -> Option<RemoteExecStream> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with('{')
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        let action = value.get("Action").and_then(|a| a.as_str())?;
        let test = value.get("Test").and_then(|t| t.as_str())?;
        let pkg = value.get("Package").and_then(|p| p.as_str()).unwrap_or("");
        let name = if pkg.is_empty() {
            test.to_string()
        } else {
            format!("{pkg}.{test}")
        };
        let duration_ms = value
            .get("Elapsed")
            .and_then(|e| e.as_f64())
            .map(|s| (s * 1000.0) as u64);

        return match action {
            "run" => Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Started {
                name,
            })),
            "pass" => Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed {
                name,
                duration_ms,
            })),
            "fail" => Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed {
                name,
                duration_ms,
                message: None,
                assertion_diff: None,
                backtrace: None,
                output: None,
            })),
            "skip" => Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped {
                name,
                reason: None,
            })),
            _ => None,
        };
    }

    // Standard human-readable `go test` text lines:
    // "=== RUN   TestFoo"
    // "--- PASS: TestFoo (0.01s)"
    // "--- FAIL: TestBar (0.05s)"
    // "--- SKIP: TestBaz (0.00s)"
    // "--- BENCH: BenchmarkFoo (0.00s)"
    // Summary lines such as "PASS", "FAIL", "ok  \tpkg\t0.012s", "FAIL\tpkg\t0.015s" are ignored.
    if let Some(rest) = trimmed.strip_prefix("=== RUN") {
        let name = rest.trim().to_string();
        if !name.is_empty() {
            return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Started {
                name,
            }));
        }
    } else if let Some(rest) = trimmed.strip_prefix("--- PASS:") {
        let (name, duration_ms) = parse_go_raw_test_suffix(rest);
        return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed {
            name,
            duration_ms,
        }));
    } else if let Some(rest) = trimmed.strip_prefix("--- FAIL:") {
        let (name, duration_ms) = parse_go_raw_test_suffix(rest);
        return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed {
            name,
            duration_ms,
            message: None,
            assertion_diff: None,
            backtrace: None,
            output: None,
        }));
    } else if let Some(rest) = trimmed.strip_prefix("--- SKIP:") {
        let (name, _) = parse_go_raw_test_suffix(rest);
        return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped {
            name,
            reason: None,
        }));
    } else if let Some(rest) = trimmed.strip_prefix("--- BENCH:") {
        let (name, _) = parse_go_raw_test_suffix(rest);
        return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Bench {
            name,
            estimate: "bench".to_string(),
            range: None,
        }));
    }

    None
}

fn parse_go_raw_test_suffix(rest: &str) -> (String, Option<u64>) {
    let rest = rest.trim();
    if let Some((name, dur_part)) = rest.rsplit_once(" (") {
        let dur_ms = dur_part
            .strip_suffix("s)")
            .or_else(|| dur_part.strip_suffix(')'))
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map(|s| (s * 1000.0) as u64);
        (name.trim().to_string(), dur_ms)
    } else {
        (rest.to_string(), None)
    }
}
