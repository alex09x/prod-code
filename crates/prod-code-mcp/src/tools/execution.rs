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
use std::net::SocketAddr;
use std::path::Path;

use super::dispatch_tool;
use super::inspection_handlers::with_indexing_notes;
use super::references::references_across;
use super::schemas::SYMBOL_ADDRESSABLE;
use super::workspace_exec_lock;
pub(crate) use super::workspace_writer::workspace_mutation_lock;
use super::workspace_writer::{WorkspaceMutationGuard, workspace_writer_gate};
use crate::protocol::McpToolCallResult;

mod budgets;
mod context;
mod positions;
mod symbol_resolution;
use self::budgets::{
    is_timeout_error, references_budget, references_busy_message,
    references_subrequest_timeout_message,
};
use self::context::resolve_workspace_root;
pub(crate) use self::context::routed_path;
use self::context::{
    definition_budget, definition_busy_message, definition_subrequest_timeout_message,
};
use positions::validate_position_arguments;
use symbol_resolution::{
    SymbolResolution, is_multi_candidate_navigation_tool, resolve_symbol_args,
};

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
/// Returns `true` when `tool_name` with `args` is guaranteed not to mutate workspace files on
/// disk and can safely execute concurrently with other read-only tool calls.
pub(crate) fn is_read_only_tool(tool_name: &str, args: &serde_json::Value) -> bool {
    match tool_name {
        "code_definition"
        | "code_references"
        | "code_callers"
        | "code_callees"
        | "code_implementations"
        | "code_supertypes"
        | "code_hover"
        | "code_type_at"
        | "code_outline"
        | "code_symbols"
        | "code_source"
        | "code_search"
        | "code_grep"
        | "code_slice"
        | "code_impact"
        | "code_diagnostics"
        | "code_validate_edit"
        | "code_validate_edits"
        | "code_assists"
        | "code_dead_code"
        | "code_dependencies"
        | "code_find_duplicates"
        | "code_structural_search"
        | "code_propose_expression"
        | "code_status"
        | "code_report_issue"
        | "code_test"
        | "code_benchmarks"
        | "code_coverage"
        | "code_diagnose_failure" => true,
        "code_shadow_run" => args.get("apply").and_then(|v| v.as_bool()) != Some(true),
        "code_pull" => args.get("list").and_then(|v| v.as_bool()) == Some(true),
        "code_check" | "code_lint" => args.get("fix").and_then(|v| v.as_bool()) != Some(true),
        _ => false,
    }
}

/// A `verify` mode is `compile` or absent. A typo such as `compiler` used to read as "no
/// compiler gate" and let an apply write unchecked (#1453); it is refused before planning.
/// `code_generate_fixture` takes a boolean `verify`, which is left to its handler.
fn validate_verify_mode(args: &serde_json::Value) -> Result<()> {
    match args.get("verify") {
        Some(serde_json::Value::String(mode)) if mode != "compile" => anyhow::bail!(
            "`verify: {mode:?}` is not a verification mode: the only one is \"compile\"; omit \
             `verify` for the analyzer check alone. Nothing was planned or written"
        ),
        _ => Ok(()),
    }
}

/// Execute an MCP tool call against the remote gateway. Keep dispatch state on the heap so
/// callers can compose many tool calls without growing their own future by every dispatch.
pub fn execute_tool<'a>(
    remote: SocketAddr,
    workspace_root: &'a Path,
    tool_name: &'a str,
    args: serde_json::Value,
) -> impl std::future::Future<Output = Result<McpToolCallResult>> + Send + 'a {
    let configured_workspace_root = workspace_root.to_path_buf();
    let budget = definition_budget(tool_name, &args);
    let reference_budget = references_budget(tool_name, &args);
    Box::pin(async move {
        let run = execute_tool_inner(
            remote,
            &configured_workspace_root,
            tool_name,
            args,
            reference_budget,
        );
        match budget {
            Some(budget) => match tokio::time::timeout_at(budget.deadline, run).await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(error)) if is_timeout_error(&error) => Ok(McpToolCallResult::error(
                    definition_subrequest_timeout_message(budget),
                )),
                Ok(Err(error)) => Err(error),
                Err(_) => Ok(McpToolCallResult::error(definition_busy_message(budget))),
            },
            None => run.await,
        }
    })
}

fn execute_tool_inner<'a>(
    remote: SocketAddr,
    configured_workspace_root: &'a Path,
    tool_name: &'a str,
    args: serde_json::Value,
    reference_budget: Option<self::context::DefinitionBudget>,
) -> impl std::future::Future<Output = Result<McpToolCallResult>> + Send + 'a {
    Box::pin(async move {
        let mut reference_budget = reference_budget;
        let mut args = args;
        let workspace_root =
            resolve_workspace_root(&configured_workspace_root, tool_name, &mut args)?;
        let workspace_root = workspace_root.as_path();
        // Invalid explicit positions are refused before symbol lookup, routing or any write.
        validate_position_arguments(tool_name, &args)?;
        validate_verify_mode(&args)?;
        let mut args = args;
        if SYMBOL_ADDRESSABLE.contains(&tool_name) {
            let batch_symbols = super::batch::extract_batch_symbols(&args);
            if batch_symbols.len() > 1 && is_multi_candidate_navigation_tool(tool_name) {
                return super::batch::execute_batch_symbols(
                    remote,
                    workspace_root,
                    tool_name,
                    args,
                    &batch_symbols,
                )
                .await;
            } else if batch_symbols.len() == 1
                && args.get("symbol").and_then(|v| v.as_str()).is_none()
            {
                if let Some(obj) = args.as_object_mut() {
                    obj.remove("symbols");
                    obj.insert(
                        "symbol".into(),
                        serde_json::Value::String(batch_symbols[0].clone()),
                    );
                }
            }
        }
        // Notes left by an earlier call are not this answer's (#391).
        let _ = crate::session::take_indexing_notes(workspace_root);
        if tool_name == "code_references"
            && let Some(dirs) = args.get("also_in").and_then(|v| v.as_array())
            && !dirs.is_empty()
        {
            let dirs = dirs.clone();
            let request = references_across(remote, workspace_root, args, &dirs);
            return match reference_budget {
                Some(budget) => match tokio::time::timeout_at(budget.deadline, request).await {
                    Ok(Ok(result)) => Ok(result),
                    Ok(Err(error)) if is_timeout_error(&error) => Ok(McpToolCallResult::error(
                        references_subrequest_timeout_message(budget),
                    )),
                    Ok(Err(error)) => Err(error),
                    Err(_) => Ok(McpToolCallResult::error(references_busy_message(budget))),
                },
                None => request.await,
            };
        }
        let read_only = is_read_only_tool(tool_name, &args);
        let workspace_command = matches!(
            tool_name,
            "code_exec"
                | "code_check"
                | "code_lint"
                | "code_test"
                | "code_benchmarks"
                | "code_coverage"
                | "code_diagnose_failure"
        );
        let _exec_guard = if !read_only || workspace_command {
            Some(workspace_exec_lock(workspace_root).lock_owned().await)
        } else {
            None
        };
        // Commands may write back files, so serialize them with other writers even while
        // readers proceed during the remote execution phase.
        let _writer = if read_only {
            None
        } else {
            Some(workspace_writer_gate(workspace_root).lock_owned().await)
        };
        let lock = workspace_mutation_lock(workspace_root);
        let _guard = if tool_name == "code_exec" {
            None // its writeback takes this lock only while applying changes
        } else if read_only {
            Some(WorkspaceMutationGuard::Read(lock.read_owned().await))
        } else {
            Some(WorkspaceMutationGuard::Write(lock.write_owned().await))
        };
        // A pull names checkout-relative artifact paths that need not exist locally (#1145).
        if tool_name == "code_pull" {
            return dispatch_tool(remote, workspace_root, tool_name, args).await;
        }
        // Normalize path parameter across aliases: path, file_path, file, package, crate (#673, #900)
        let mut args = args;
        let initial_path = args
            .get("path")
            .or_else(|| args.get("file_path"))
            .or_else(|| args.get("file"))
            .or_else(|| args.get("package"))
            .or_else(|| args.get("crate"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if let Some(ref p) = initial_path
            && let Some(obj) = args.as_object_mut()
            && !obj.contains_key("path")
        {
            obj.insert("path".into(), serde_json::Value::String(p.clone()));
        }

        // A path in a nested project of another language goes to a node that serves it (#125, #673).
        // Route before resolving symbols so cross-language symbol lookups hit the right language engine.
        let remote = crate::cluster::route_for_path(
            remote,
            workspace_root,
            routed_path(tool_name, initial_path.as_deref()),
        );
        let remote = match reference_budget {
            Some(budget) => match tokio::time::timeout_at(budget.deadline, remote).await {
                Ok(result) => result?,
                Err(_) => return Ok(McpToolCallResult::error(references_busy_message(budget))),
            },
            None => remote.await?,
        };

        let args =
            match resolve_symbol_args(remote, workspace_root, tool_name, args, reference_budget)
                .await?
            {
                SymbolResolution::Dispatch(args, budget) => {
                    reference_budget = budget.or(reference_budget);
                    args
                }
                SymbolResolution::Respond(result) => return Ok(result),
            };
        // Resolved symbol positions obey the same contract, including a supplied selection end.
        validate_position_arguments(tool_name, &args)?;
        // A path in a nested project of another language goes to a node that serves it (#125).
        let final_path = args
            .get("path")
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        let remote = crate::cluster::route_for_path(
            remote,
            workspace_root,
            routed_path(tool_name, final_path.as_deref()),
        );
        let remote = match reference_budget {
            Some(budget) => match tokio::time::timeout_at(budget.deadline, remote).await {
                Ok(result) => result?,
                Err(_) => return Ok(McpToolCallResult::error(references_busy_message(budget))),
            },
            None => remote.await?,
        };
        let dispatch = dispatch_tool(remote, workspace_root, tool_name, args);
        let result = match reference_budget {
            Some(budget) => match tokio::time::timeout_at(budget.deadline, dispatch).await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(error)) if is_timeout_error(&error) => Ok(McpToolCallResult::error(
                    references_subrequest_timeout_message(budget),
                )),
                Ok(Err(error)) => Err(error),
                Err(_) => Ok(McpToolCallResult::error(references_busy_message(budget))),
            },
            None => dispatch.await,
        };
        with_indexing_notes(result, workspace_root)
    })
}
