/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::ShadowOutcome;

/// One text for agents and the CLI: every hypothesis on a line, the winner's diff, the tail
/// of every failing hypothesis's output.
pub fn render_report(
    outcome: &ShadowOutcome,
    command: &[String],
    applied: Option<&[String]>,
    failure_tail_chars: usize,
) -> String {
    let mut text = format!(
        "$ {}   ({} hypothesis(es), {} mode, on {})\n",
        command.join(" "),
        outcome.results.len(),
        outcome.mode,
        outcome.server_workspace_root
    );
    let width = outcome
        .results
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    for &i in &outcome.ranking {
        let r = &outcome.results[i];
        let status = match (&r.error, r.timed_out, r.exit_code) {
            (Some(err), _, _) => format!("failed: {err}"),
            (None, true, _) => "timed out".to_string(),
            (None, false, Some(code)) => format!("exit {code}"),
            (None, false, None) => "killed".to_string(),
        };
        let tests = match r.tests {
            Some((p, f)) => format!("  {p} passed, {f} failed"),
            None => String::new(),
        };
        let mark = if outcome.winner == Some(i) {
            "  <- winner"
        } else {
            ""
        };
        text.push_str(&format!(
            "  {:width$}  {status:<10} in {:>6.1}s{tests}  ~{} changed line(s){mark}\n",
            r.name,
            r.duration_ms as f64 / 1000.0,
            r.changed_lines,
            width = width
        ));
    }
    match outcome.winner {
        Some(i) => {
            let r = &outcome.results[i];
            text.push_str(&format!("winner: {}\n", r.name));
            if r.diff.is_empty() {
                text.push_str("(no difference from the checkout)\n");
            } else {
                text.push_str(&r.diff);
                if !r.diff.ends_with('\n') {
                    text.push('\n');
                }
            }
            if let Some(files) = applied {
                text.push_str(&format!(
                    "[applied {} file(s) to the checkout: {}]\n",
                    files.len(),
                    files.join(", ")
                ));
            }
        }
        None => {
            if let Some(&best) = outcome.ranking.first() {
                text.push_str(&format!(
                    "no hypothesis passed; closest: {}\n",
                    outcome.results[best].name
                ));
            }
        }
    }
    for &i in &outcome.ranking {
        let r = &outcome.results[i];
        if r.passed() || r.output.is_empty() {
            continue;
        }
        // Parallel cargos wait on the shared package-cache lock; that noise is not a finding.
        let output: String = r
            .output
            .lines()
            .filter(|l| !l.trim_start().starts_with("Blocking waiting for file lock"))
            .collect::<Vec<_>>()
            .join("\n");
        let (tail, shown_len) = format_failure_output(&output, failure_tail_chars);
        text.push_str(&format!(
            "--- {} output (last {} of {} bytes) ---\n{}\n",
            r.name,
            shown_len,
            r.output_len,
            tail.trim_end()
        ));
    }
    text.trim_end().to_string()
}

fn is_diagnostic_error_header(line: &str) -> bool {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("error") {
        let is_code = rest.starts_with('[') && rest.contains("]:");
        let is_colon = rest.starts_with(':');
        if is_code || is_colon {
            let msg = rest.split_once(':').map_or("", |(_, m)| m.trim());
            if !msg.starts_with("could not compile")
                && !msg.starts_with("aborting due to")
                && !msg.starts_with("build failed")
            {
                return true;
            }
        }
    }
    if trimmed.contains(": error:") || trimmed.contains(" - error TS") {
        return true;
    }
    (trimmed.starts_with("---- ") && trimmed.ends_with(" stdout ----"))
        || (trimmed.starts_with("thread '") && trimmed.contains("' panicked at "))
        || trimmed.starts_with("--- FAIL: ")
        || trimmed.starts_with("FAILED ")
}

fn is_diagnostic_boundary(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("warning:")
        || (trimmed.starts_with("warning[") && trimmed.contains("]:"))
        || trimmed.contains(": warning:")
        || trimmed.contains(" - warning TS")
        || is_diagnostic_error_header(line)
        || trimmed.starts_with("Compiling ")
        || trimmed.starts_with("Checking ")
        || trimmed.starts_with("Finished ")
        || trimmed.starts_with("error: could not compile")
        || trimmed.starts_with("error: aborting due to")
}

fn extract_error_blocks(output: &str) -> Vec<String> {
    let lines: Vec<&str> = output.lines().collect();
    let mut blocks = Vec::new();
    let mut idx = 0;
    while idx < lines.len() {
        if is_diagnostic_error_header(lines[idx]) {
            let mut block = Vec::new();
            block.push(lines[idx]);
            idx += 1;
            while idx < lines.len() && !is_diagnostic_boundary(lines[idx]) {
                block.push(lines[idx]);
                idx += 1;
            }
            let text = block.join("\n").trim().to_string();
            if !text.is_empty() {
                blocks.push(text);
            }
        } else {
            idx += 1;
        }
    }
    blocks
}

/// Formats the failure output within `limit_chars`, ensuring compiler errors and failure
/// diagnostics are preserved even when warning output exceeds the tail cap (#794).
fn format_failure_output(output: &str, limit_chars: usize) -> (String, usize) {
    let chars: Vec<char> = output.chars().collect();
    if chars.len() <= limit_chars {
        return (output.to_string(), output.len());
    }

    let error_blocks = extract_error_blocks(output);
    if error_blocks.is_empty() {
        let start = chars.len().saturating_sub(limit_chars);
        let tail: String = chars[start..].iter().collect();
        let len = tail.len();
        return (tail, len);
    }

    // Check if the standard tail already contains all error blocks.
    let tail_start = chars.len().saturating_sub(limit_chars);
    let tail_str: String = chars[tail_start..].iter().collect();
    if error_blocks.iter().all(|b| tail_str.contains(b.as_str())) {
        let len = tail_str.len();
        return (tail_str, len);
    }

    // Errors occurred earlier in the stream and would be dropped by raw tail truncation.
    // Retain error blocks, then fill remaining budget with the tail.
    let errors_combined = error_blocks.join("\n\n");
    let err_chars: Vec<char> = errors_combined.chars().collect();
    if err_chars.len() >= limit_chars {
        let truncated: String = err_chars[..limit_chars].iter().collect();
        let len = truncated.len();
        return (truncated, len);
    }

    let separator = "\n\n[... output omitted ...]\n\n";
    let sep_len = separator.chars().count();
    let remaining = limit_chars.saturating_sub(err_chars.len() + sep_len);
    if remaining > 100 {
        let tail_part_start = chars.len().saturating_sub(remaining);
        let tail_part: String = chars[tail_part_start..].iter().collect();
        let combined = format!("{errors_combined}{separator}{tail_part}");
        let len = combined.len();
        (combined, len)
    } else {
        let len = errors_combined.len();
        (errors_combined, len)
    }
}
