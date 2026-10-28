/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::time::Duration;

const DEFAULT_DEFINITION_BUDGET_MS: u64 = 120_000;
const MAX_DEFINITION_BUDGET_MS: u64 = 120_000;

#[derive(Clone, Copy)]
pub(crate) struct DefinitionBudget {
    pub(crate) deadline: tokio::time::Instant,
    pub(crate) duration: Duration,
}

pub(crate) fn definition_budget(
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<DefinitionBudget> {
    if tool_name != "code_definition" {
        return None;
    }
    let milliseconds = args
        .get("budget_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or(DEFAULT_DEFINITION_BUDGET_MS)
        .clamp(1, MAX_DEFINITION_BUDGET_MS);
    let duration = Duration::from_millis(milliseconds);
    Some(DefinitionBudget {
        deadline: tokio::time::Instant::now() + duration,
        duration,
    })
}

pub(crate) fn definition_busy_message(budget: DefinitionBudget) -> String {
    format!(
        "code_definition did not finish within {} ms; the analyzer may be busy indexing or compiling. Retry, or pass a specific path and position.",
        budget.duration.as_millis()
    )
}

pub(crate) fn definition_subrequest_timeout_message(budget: DefinitionBudget) -> String {
    format!(
        "A code_definition LSP request timed out before the shared {} ms budget completed; the analyzer may be busy indexing or compiling. Retry or pass a specific path and position.",
        budget.duration.as_millis()
    )
}

pub(crate) fn is_timeout_error(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    message.contains("timeout") || message.contains("timed out") || message.contains("deadline")
}

pub(crate) fn is_multi_candidate_navigation_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "code_definition"
            | "code_references"
            | "code_hover"
            | "code_type_at"
            | "code_callers"
            | "code_callees"
            | "code_implementations"
            | "code_supertypes"
    )
}

pub(crate) fn checked_position_argument(value: &serde_json::Value, name: &str) -> Result<u32> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .with_context(|| {
            format!(
                "'{name}' must be a one-based coordinate in 1..={}",
                u32::MAX
            )
        })
}

/// A `verify` mode is `compile` or absent. A typo such as `compiler` used to read as "no
/// compiler gate" and let an apply write unchecked (#1453); it is refused before planning.
/// `code_generate_fixture` takes a boolean `verify`, which is left to its handler.
pub(crate) fn validate_verify_mode(args: &serde_json::Value) -> Result<()> {
    match args.get("verify") {
        Some(serde_json::Value::String(mode)) if mode != "compile" => anyhow::bail!(
            "`verify: {mode:?}` is not a verification mode: the only one is \"compile\"; omit \
             `verify` for the analyzer check alone. Nothing was planned or written"
        ),
        _ => Ok(()),
    }
}

/// The path a tool is routed by. A text search reads the synced copy, which every node holding
/// the workspace has: a Markdown file needs no node that serves Markdown, nor a Swift one a Mac
/// (#1558).
pub(crate) fn routed_path<'a>(tool_name: &str, path: Option<&'a str>) -> Option<&'a str> {
    (tool_name != "code_grep").then_some(path).flatten()
}
