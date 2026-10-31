/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use serde_json::Value;
use std::path::Path;

use super::super::compile_gate::resolve_file_path;
use super::super::dispatch_tool;
use super::super::inspection_handlers::with_indexing_notes;
use super::super::schemas::SYMBOL_ADDRESSABLE;
use super::super::symbols::{
    DEFAULT_AMBIGUOUS_CANDIDATE_LIMIT, resolve_symbol, resolve_symbol_candidates,
};
use super::budgets::{
    DefinitionBudget, references_busy_message, references_subrequest_timeout_message,
};
use super::validate_position_arguments;
use crate::protocol::{McpContentItem, McpToolCallResult};

pub(super) enum SymbolResolution {
    Dispatch(Value, Option<DefinitionBudget>),
    Respond(McpToolCallResult),
}

pub(super) fn is_multi_candidate_navigation_tool(tool_name: &str) -> bool {
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

pub(super) async fn resolve_symbol_args(
    remote: std::net::SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: Value,
    reference_budget: Option<DefinitionBudget>,
) -> Result<SymbolResolution> {
    // An explicit path and line select the exact declaration for code_wrap_return. Resolving its
    // duplicate name through the workspace index would discard that selection and reject ties.
    let explicit_wrap_return_position = tool_name == "code_wrap_return"
        && args
            .get("path")
            .and_then(Value::as_str)
            .is_some_and(|path| !path.trim().is_empty())
        && args.get("line").and_then(Value::as_u64).is_some();
    if !SYMBOL_ADDRESSABLE.contains(&tool_name) || explicit_wrap_return_position {
        return Ok(SymbolResolution::Dispatch(args, None));
    }
    let Some(symbol) = args.get("symbol").and_then(Value::as_str) else {
        return Ok(SymbolResolution::Dispatch(args, None));
    };
    let symbol_trimmed = symbol.trim().to_string();
    if symbol_trimmed.is_empty() {
        return Ok(SymbolResolution::Dispatch(args, None));
    }

    let hint = args
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(|path| resolve_file_path(workspace_root, path));
    let lookup = async {
        if is_multi_candidate_navigation_tool(tool_name) {
            let max_candidates = args
                .get("max_candidates")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(DEFAULT_AMBIGUOUS_CANDIDATE_LIMIT);
            Box::pin(resolve_symbol_candidates(
                remote,
                workspace_root,
                &symbol_trimmed,
                hint.as_deref(),
                max_candidates,
            ))
            .await
        } else {
            Ok(vec![
                Box::pin(resolve_symbol(
                    remote,
                    workspace_root,
                    &symbol_trimmed,
                    hint.as_deref(),
                ))
                .await?,
            ])
        }
    };
    let candidates = match (reference_budget, tool_name == "code_references") {
        (Some(budget), true) => match tokio::time::timeout_at(budget.deadline, lookup).await {
            Ok(Ok(candidates)) => candidates,
            Ok(Err(error)) if super::budgets::is_timeout_error(&error) => {
                return Ok(SymbolResolution::Respond(McpToolCallResult::error(
                    references_subrequest_timeout_message(budget),
                )));
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Ok(SymbolResolution::Respond(McpToolCallResult::error(
                    references_busy_message(budget),
                )));
            }
        },
        _ => lookup.await?,
    };

    if candidates.len() > 1 {
        if tool_name == "code_definition" {
            return Ok(SymbolResolution::Respond(McpToolCallResult::text(
                super::super::symbols::matching::ambiguous_candidates_error(
                    workspace_root,
                    &symbol_trimmed,
                    &candidates,
                )
                .to_string(),
            )));
        }
        return Ok(SymbolResolution::Respond(
            resolve_multiple_candidates(
                remote,
                workspace_root,
                tool_name,
                args,
                candidates,
                reference_budget.filter(|_| tool_name == "code_references"),
            )
            .await?,
        ));
    }

    let hit = &candidates[0];
    let mut owned = args;
    if let Some(obj) = owned.as_object_mut() {
        obj.insert(
            "path".into(),
            Value::String(hit.path.to_string_lossy().into_owned()),
        );
        obj.insert("line".into(), serde_json::json!(hit.line));
        obj.insert("character".into(), serde_json::json!(hit.col));
    }
    Ok(SymbolResolution::Dispatch(
        owned,
        reference_budget.filter(|_| tool_name == "code_references"),
    ))
}

async fn resolve_multiple_candidates(
    remote: std::net::SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: Value,
    candidates: Vec<crate::tools::symbols::SymbolHit>,
    reference_budget: Option<DefinitionBudget>,
) -> Result<McpToolCallResult> {
    let total = candidates.len();
    let shared_budget = reference_budget
        .map(|budget| budget.duration)
        .unwrap_or_else(|| {
            if let Some(ms) = args.get("budget_ms").and_then(Value::as_u64) {
                std::time::Duration::from_millis(ms.max(1))
            } else if let Some(secs) = args
                .get("budget_secs")
                .or_else(|| args.get("timeout_secs"))
                .and_then(Value::as_u64)
            {
                std::time::Duration::from_secs(secs.clamp(1, 90))
            } else {
                crate::call_tree::DEFAULT_CALL_TREE_BUDGET
            }
        });
    let deadline = reference_budget
        .map(|budget| budget.deadline.into_std())
        .unwrap_or_else(|| std::time::Instant::now() + shared_budget);
    let mut out = format!(
        "`{}` matched {total} candidates:\n",
        args.get("symbol")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
    );
    let mut any_ok = false;
    for (idx, hit) in candidates.iter().enumerate() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            out.push_str(&format!(
                "\n[Partial result: shared server time budget ({:.1}s) expired after {idx}/{total} candidates; pass `path` or a more specific symbol to inspect remaining candidates.]\n",
                shared_budget.as_secs_f64()
            ));
            break;
        }
        let mut candidate_args = args.clone();
        if let Some(obj) = candidate_args.as_object_mut() {
            obj.insert(
                "path".into(),
                Value::String(hit.path.to_string_lossy().into_owned()),
            );
            obj.insert("line".into(), serde_json::json!(hit.line));
            obj.insert("character".into(), serde_json::json!(hit.col));
            obj.insert(
                "budget_ms".into(),
                serde_json::json!((remaining.as_millis() as u64).max(1)),
            );
        }
        validate_position_arguments(tool_name, &candidate_args)?;
        let candidate_remote = crate::cluster::route_for_path(
            remote,
            workspace_root,
            candidate_args.get("path").and_then(Value::as_str),
        )
        .await?;
        out.push_str(&format!(
            "\n=== [{}/{total}] {} ===\n",
            idx + 1,
            hit.render(workspace_root)
        ));
        match tokio::time::timeout(
            remaining,
            Box::pin(dispatch_tool(
                candidate_remote,
                workspace_root,
                tool_name,
                candidate_args,
            )),
        )
        .await
        {
            Ok(Ok(result)) => {
                if !result.is_error {
                    any_ok = true;
                }
                for item in &result.content {
                    let McpContentItem::Text { text } = item;
                    out.push_str(text.trim_end());
                    out.push('\n');
                }
            }
            Ok(Err(error)) => out.push_str(&format!("Error: {error:#}\n")),
            Err(_) => {
                out.push_str(&format!(
                    "[Timed out on candidate {}/{total} after {:.1}s shared server budget; returning partial multi-candidate result.]\n",
                    idx + 1,
                    shared_budget.as_secs_f64()
                ));
                if idx + 1 < total {
                    out.push_str(&format!(
                        "\n[Partial result: shared server time budget ({:.1}s) expired at candidate {}/{total}; pass `path` or a more specific symbol to inspect remaining candidates.]\n",
                        shared_budget.as_secs_f64(),
                        idx + 1,
                    ));
                }
                break;
            }
        }
    }
    let result = if any_ok {
        McpToolCallResult::text(out.trim_end().to_string())
    } else {
        McpToolCallResult::error(out.trim_end().to_string())
    };
    with_indexing_notes(Ok(result), workspace_root)
}
