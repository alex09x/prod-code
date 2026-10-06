/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::HypothesisOutcome;
use crate::verify;

/// Best first: passed before failed, then fewer failing tests, more passing tests, a smaller
/// diff, and finally the order the hypotheses were given in.
pub fn rank(results: &[HypothesisOutcome]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..results.len()).collect();
    order.sort_by_key(|&i| {
        let r = &results[i];
        let (passed, failed) = r.tests.unwrap_or((0, 0));
        (
            !r.passed(),
            failed,
            std::cmp::Reverse(passed),
            r.changed_lines,
            i,
        )
    });
    order
}

/// (passed, failed) parsed from the output of a known test runner, chosen by the command.
pub fn test_counts(command: &[String], output: &str) -> Option<(u64, u64)> {
    let program = command
        .first()
        .map(|p| p.rsplit('/').next().unwrap_or(p))
        .unwrap_or("");
    let has = |word: &str| command.iter().skip(1).any(|a| a == word);
    let joined = command.join(" ");
    let (passed, failed, _) = match program {
        "cargo" if has("test") || has("nextest") => verify::parse_cargo_test_text(output),
        "go" if has("test") && has("-json") => verify::parse_go_test_json(output),
        "pytest" | "py.test" => verify::parse_pytest_text(output),
        "python" | "python3" if joined.contains("pytest") => verify::parse_pytest_text(output),
        "python" | "python3" if joined.contains("unittest") => verify::parse_unittest_text(output),
        "vitest" => verify::parse_vitest_text(output),
        "jest" => verify::parse_jest_text(output),
        "bun" if has("test") => verify::parse_bun_test_text(output),
        "npx" | "npm" | "pnpm" | "yarn" if joined.contains("vitest") => {
            verify::parse_vitest_text(output)
        }
        "npx" | "npm" | "pnpm" | "yarn" if joined.contains("jest") => {
            verify::parse_jest_text(output)
        }
        "swift" if has("test") => verify::parse_xctest_text(output),
        "xcodebuild" => verify::parse_xctest_text(output),
        "ctest" => verify::parse_ctest_text(output),
        "meson" if has("test") => verify::parse_meson_test_text(output),
        _ => return None,
    };
    Some((passed, failed))
}
