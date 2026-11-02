/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::Value;

/// Collect metadata from operation results and completed iteration items, never source text.
pub(super) fn warnings(value: &Value) -> Vec<&'static str> {
    let mut output = Vec::new();
    collect(value, &mut output);
    output
}

fn collect(value: &Value, output: &mut Vec<&'static str>) {
    if let Some(items) = value.as_array() {
        for item in items {
            collect(item, output);
        }
        return;
    }
    for (present, warning) in [
        (
            value
                .get("indexing_notes")
                .and_then(Value::as_array)
                .is_some_and(|notes| !notes.is_empty()),
            "index_may_be_incomplete",
        ),
        (
            value.get("truncated").and_then(Value::as_bool) == Some(true),
            "candidate_or_match_limit",
        ),
        (
            value.get("exhaustive").and_then(Value::as_bool) == Some(false),
            "search_not_exhaustive",
        ),
        (
            value.get("incomplete").and_then(Value::as_bool) == Some(true),
            "operation_incomplete",
        ),
        (
            value.get("coverage").is_some_and(|coverage| {
                ["unexpanded", "external", "unsupported"].iter().any(|key| {
                    coverage
                        .get(key)
                        .and_then(Value::as_u64)
                        .is_some_and(|count| count > 0)
                })
            }),
            "dependency_boundary",
        ),
    ] {
        if present && !output.contains(&warning) {
            output.push(warning);
        }
    }
}

pub(super) fn partial(warnings: &[&str]) -> bool {
    warnings.iter().any(|warning| {
        matches!(
            *warning,
            "index_may_be_incomplete" | "candidate_or_match_limit" | "operation_incomplete"
        )
    })
}
