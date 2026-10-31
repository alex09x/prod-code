/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::time::Duration;

const DEFAULT_DEFINITION_BUDGET_MS: u64 = 120_000;
const MAX_DEFINITION_BUDGET_MS: u64 = 120_000;
const DEFAULT_REFERENCES_BUDGET_MS: u64 = 45_000;
const MAX_REFERENCES_BUDGET_MS: u64 = 50_000;

#[derive(Clone, Copy)]
pub(crate) struct DefinitionBudget {
    pub(crate) deadline: tokio::time::Instant,
    pub(crate) duration: Duration,
}

pub(crate) fn definition_budget(
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<DefinitionBudget> {
    if tool_name != "code_definition" || super::super::batch::extract_batch_symbols(args).len() > 1
    {
        // The batch runner apportions its own shared budget and preserves partial successes.
        return None;
    }
    let milliseconds = args
        .get("budget_ms")
        .and_then(|value| value.as_u64())
        .or_else(|| args.get("timeout_millis").and_then(|value| value.as_u64()))
        .or_else(|| {
            args.get("budget_secs")
                .or_else(|| args.get("timeout_secs"))
                .and_then(|value| value.as_u64())
                .map(|seconds| seconds.saturating_mul(1000))
        })
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

pub(super) fn references_budget(
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<DefinitionBudget> {
    if tool_name != "code_references" || super::super::batch::extract_batch_symbols(args).len() > 1
    {
        // The batch runner owns its shared budget and preserves partial symbol results.
        return None;
    }
    let milliseconds = args
        .get("budget_ms")
        .and_then(|value| value.as_u64())
        .or_else(|| args.get("timeout_millis").and_then(|value| value.as_u64()))
        .or_else(|| {
            args.get("budget_secs")
                .or_else(|| args.get("timeout_secs"))
                .and_then(|value| value.as_u64())
                .map(|seconds| seconds.saturating_mul(1000))
        })
        .unwrap_or(DEFAULT_REFERENCES_BUDGET_MS)
        .clamp(1, MAX_REFERENCES_BUDGET_MS);
    let duration = Duration::from_millis(milliseconds);
    Some(DefinitionBudget {
        deadline: tokio::time::Instant::now() + duration,
        duration,
    })
}

pub(super) fn references_busy_message(budget: DefinitionBudget) -> String {
    format!(
        "code_references did not finish within {} ms; the analyzer may be busy indexing or compiling. Retry, or pass a narrower path or symbol.",
        budget.duration.as_millis()
    )
}

pub(super) fn references_subrequest_timeout_message(budget: DefinitionBudget) -> String {
    format!(
        "A code_references LSP request timed out before the shared {} ms budget completed; the analyzer may be busy indexing or compiling. Retry or pass a narrower path or symbol.",
        budget.duration.as_millis()
    )
}

pub(super) fn is_timeout_error(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("failed to connect to remote gateway") {
        return false;
    }
    message.contains("timeout") || message.contains("timed out") || message.contains("deadline")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_timeout_uses_supported_aliases() {
        for (args, expected_ms) in [
            (serde_json::json!({ "budget_ms": 250 }), 250),
            (
                serde_json::json!({ "budget_ms": 250, "timeout_millis": 900 }),
                250,
            ),
            (serde_json::json!({ "timeout_millis": 500 }), 500),
            (serde_json::json!({ "budget_secs": 3 }), 3_000),
            (serde_json::json!({ "timeout_secs": 4 }), 4_000),
        ] {
            let actual = references_budget("code_references", &args)
                .expect("single reference call has a budget");
            assert_eq!(actual.duration, Duration::from_millis(expected_ms));
        }
    }

    #[test]
    fn references_timeout_does_not_swallow_gateway_connection_failures() {
        let error =
            anyhow::anyhow!("Failed to connect to remote gateway 127.0.0.1:9400: timed out");
        assert!(!is_timeout_error(&error));
        let lsp_error = anyhow::anyhow!("timeout waiting for references response");
        assert!(is_timeout_error(&lsp_error));
    }

    #[test]
    fn references_subrequest_timeout_names_the_early_lsp_timeout() {
        let budget = DefinitionBudget {
            deadline: tokio::time::Instant::now() + Duration::from_millis(500),
            duration: Duration::from_millis(500),
        };
        let message = references_subrequest_timeout_message(budget);
        assert!(message.contains("code_references LSP request timed out"));
        assert!(message.contains("before the shared 500 ms budget completed"));
    }

    #[test]
    fn definition_timeout_uses_supported_aliases() {
        for (args, expected_ms) in [
            (serde_json::json!({ "budget_ms": 250 }), 250),
            (
                serde_json::json!({ "budget_ms": 250, "timeout_millis": 900 }),
                250,
            ),
            (serde_json::json!({ "timeout_millis": 500 }), 500),
            (serde_json::json!({ "budget_secs": 3 }), 3_000),
            (serde_json::json!({ "timeout_secs": 4 }), 4_000),
        ] {
            let actual = definition_budget("code_definition", &args)
                .expect("single definition call has a budget");
            assert_eq!(actual.duration, Duration::from_millis(expected_ms));
        }
    }

    #[test]
    fn definition_timeout_batch_uses_its_partial_result_budget() {
        assert!(
            definition_budget(
                "code_definition",
                &serde_json::json!({
                    "symbols": ["Fast", "Slow"],
                    "budget_ms": 200
                }),
            )
            .is_none()
        );
    }
}
