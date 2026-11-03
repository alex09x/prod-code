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

/// The deadline of a single navigation call received at `received`: the budget counts from the
/// moment the request was read, so a failover retry spends what is left of it (#1814).
pub(crate) fn navigation_deadline(
    tool_name: &str,
    args: &serde_json::Value,
    received: tokio::time::Instant,
) -> Option<tokio::time::Instant> {
    definition_budget(tool_name, args, received)
        .or_else(|| references_budget(tool_name, args, received))
        .map(|budget| budget.deadline)
}

/// The answer of a call whose budget ended while `stage` was a wait in this client's own queue.
fn queued_message(tool: &str, budget: DefinitionBudget, stage: &str) -> String {
    format!(
        "{tool} did not finish within {} ms: it was still queued, {stage}, so the analyzer was \
         not asked. Retry when that call finishes.",
        budget.duration.as_millis()
    )
}

pub(crate) fn definition_budget(
    tool_name: &str,
    args: &serde_json::Value,
    received: tokio::time::Instant,
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
        deadline: received + duration,
        duration,
    })
}

pub(crate) fn definition_busy_message(
    budget: DefinitionBudget,
    sync_stage: Option<&str>,
) -> String {
    if let Some(stage) = sync_stage.filter(|stage| crate::sync::types::is_queue_stage(stage)) {
        return queued_message("code_definition", budget, stage);
    }
    if let Some(stage) = sync_stage {
        return format!(
            "code_definition did not finish within {} ms while {stage}; workspace sync did not complete before the budget expired. Check the node with `prod-code status` and retry.",
            budget.duration.as_millis()
        );
    }
    format!(
        "code_definition did not finish within {} ms; the analyzer may be busy indexing or compiling. Retry, or pass a specific path and position.",
        budget.duration.as_millis()
    )
}

pub(crate) fn definition_subrequest_timeout_message(
    budget: DefinitionBudget,
    error: &anyhow::Error,
) -> String {
    if is_sync_timeout(error) {
        return format!(
            "code_definition timed out during workspace sync before the shared {} ms budget completed: {error:#}",
            budget.duration.as_millis()
        );
    }
    format!(
        "A code_definition LSP request timed out before the shared {} ms budget completed; the analyzer may be busy indexing or compiling. Retry or pass a specific path and position.",
        budget.duration.as_millis()
    )
}

pub(super) fn references_budget(
    tool_name: &str,
    args: &serde_json::Value,
    received: tokio::time::Instant,
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
        deadline: received + duration,
        duration,
    })
}

pub(super) fn references_busy_message(
    budget: DefinitionBudget,
    sync_stage: Option<&str>,
) -> String {
    if let Some(stage) = sync_stage.filter(|stage| crate::sync::types::is_queue_stage(stage)) {
        return queued_message("code_references", budget, stage);
    }
    if let Some(stage) = sync_stage {
        return format!(
            "code_references did not finish within {} ms while {stage}; workspace sync did not complete before the budget expired. Check the node with `prod-code status` and retry.",
            budget.duration.as_millis()
        );
    }
    format!(
        "code_references did not finish within {} ms; the analyzer may be busy indexing or compiling. Retry, or pass a narrower path or symbol.",
        budget.duration.as_millis()
    )
}

pub(super) fn references_subrequest_timeout_message(
    budget: DefinitionBudget,
    error: &anyhow::Error,
) -> String {
    if is_sync_timeout(error) {
        return format!(
            "code_references timed out during workspace sync before the shared {} ms budget completed: {error:#}",
            budget.duration.as_millis()
        );
    }
    format!(
        "A code_references LSP request timed out before the shared {} ms budget completed; the analyzer may be busy indexing or compiling. Retry or pass a narrower path or symbol.",
        budget.duration.as_millis()
    )
}

pub(super) fn is_sync_timeout(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    (message.contains("timeout") || message.contains("timed out") || message.contains("deadline"))
        && (message.contains("workspace sync")
            || message.contains("sync probe")
            || message.contains("waiting for sync"))
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
            let actual = references_budget("code_references", &args, tokio::time::Instant::now())
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
        let lsp_error = anyhow::anyhow!("timeout waiting for textDocument/references");
        let message = references_subrequest_timeout_message(budget, &lsp_error);
        assert!(message.contains("code_references LSP request timed out"));
        assert!(message.contains("before the shared 500 ms budget completed"));
    }

    #[test]
    fn sync_and_probe_timeouts_preserve_sync_cause_instead_of_claiming_lsp_busy() {
        let budget = DefinitionBudget {
            deadline: tokio::time::Instant::now() + Duration::from_millis(120_000),
            duration: Duration::from_millis(120_000),
        };
        let probe_timeout = anyhow::anyhow!(
            "timed out after 30s waiting for sync probe response from gateway 127.0.0.1:9400"
        )
        .context("workspace sync on the open session failed");
        assert!(is_timeout_error(&probe_timeout));
        assert!(is_sync_timeout(&probe_timeout));

        let def_msg = definition_subrequest_timeout_message(budget, &probe_timeout);
        assert!(
            def_msg.contains("timed out during workspace sync"),
            "{def_msg}"
        );
        assert!(
            def_msg.contains("waiting for sync probe response"),
            "{def_msg}"
        );
        assert!(!def_msg.contains("LSP request timed out"), "{def_msg}");
        assert!(
            !def_msg.contains("the analyzer may be busy indexing or compiling"),
            "{def_msg}"
        );

        let ref_msg = references_subrequest_timeout_message(budget, &probe_timeout);
        assert!(
            ref_msg.contains("timed out during workspace sync"),
            "{ref_msg}"
        );
        assert!(
            ref_msg.contains("waiting for sync probe response"),
            "{ref_msg}"
        );
        assert!(!ref_msg.contains("LSP request timed out"), "{ref_msg}");

        let busy_sync = definition_busy_message(budget, Some("waiting for sync probe response"));
        assert!(
            busy_sync.contains("waiting for sync probe response")
                && !busy_sync.contains("the analyzer may be busy indexing or compiling"),
            "{busy_sync}"
        );
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
            let actual = definition_budget("code_definition", &args, tokio::time::Instant::now())
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
                tokio::time::Instant::now(),
            )
            .is_none()
        );
    }
}
