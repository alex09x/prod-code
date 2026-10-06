/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::BenchResult;

/// The benchmark results in a run's output: criterion's `name time: [low estimate high]` (the
/// name on the line before when it is long), libtest's `test name ... bench: N ns/iter (+/- M)`
/// and Go's `BenchmarkName-8  N  T ns/op`.
pub fn parse_bench_text(text: &str) -> Vec<BenchResult> {
    let mut out = Vec::new();
    let mut previous = "";
    for line in text.lines() {
        let t = line.trim();
        if let Some(at) = t.find("time:") {
            let inner = t[at + 5..]
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']');
            let parts: Vec<&str> = inner.split_whitespace().collect();
            let name = t[..at].trim();
            let name = if name.is_empty() { previous } else { name };
            if parts.len() == 6 && !name.is_empty() {
                out.push(BenchResult {
                    name: name.to_string(),
                    estimate: format!("{} {}", parts[2], parts[3]),
                    range: Some(format!(
                        "{} {} .. {} {}",
                        parts[0], parts[1], parts[4], parts[5]
                    )),
                });
            }
        } else if let Some(rest) = t.strip_prefix("test ")
            && let Some((name, result)) = rest.split_once(" ... bench:")
        {
            let result = result.trim();
            let (estimate, range) = match result.split_once('(') {
                Some((e, r)) => (e.trim(), Some(r.trim_end_matches(')').trim().to_string())),
                None => (result, None),
            };
            out.push(BenchResult {
                name: name.trim().to_string(),
                estimate: estimate.replace(',', ""),
                range,
            });
        } else if t.starts_with("Benchmark") {
            let words: Vec<&str> = t.split_whitespace().collect();
            if words.len() >= 4 && words[3].ends_with("/op") {
                out.push(BenchResult {
                    name: words[0].to_string(),
                    estimate: format!("{} {}", words[2], words[3]),
                    range: None,
                });
            }
        }
        if !t.is_empty() && !t.starts_with("Benchmarking") {
            previous = t;
        }
    }
    out
}
