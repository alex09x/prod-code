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

use super::compile_gate::resolve_file_path;
use super::dispatch_tool;
use super::inspection_handlers::with_indexing_notes;
use super::references::references_across;
use super::schemas::SYMBOL_ADDRESSABLE;
use super::symbols::{
    DEFAULT_AMBIGUOUS_CANDIDATE_LIMIT, resolve_symbol, resolve_symbol_candidates,
};
use super::workspace_exec_lock;
pub(crate) use super::workspace_writer::workspace_mutation_lock;
use super::workspace_writer::{WorkspaceMutationGuard, workspace_writer_gate};
use crate::protocol::McpToolCallResult;

mod context;
mod positions;
use self::context::resolve_workspace_root;
pub(crate) use self::context::routed_path;
use positions::validate_position_arguments;

fn is_multi_candidate_navigation_tool(tool_name: &str) -> bool {
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
    Box::pin(async move {
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
            return references_across(remote, workspace_root, args, &dirs).await;
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
        )
        .await?;

        // `symbol` instead of line/character: resolve the name through the workspace symbol
        // index, then run the tool at that position.
        let args = if SYMBOL_ADDRESSABLE.contains(&tool_name)
            && let Some(symbol) = args.get("symbol").and_then(|v| v.as_str())
            && !symbol.trim().is_empty()
        {
            let symbol_trimmed = symbol.trim().to_string();
            let hint = args
                .get("path")
                .and_then(|v| v.as_str())
                .filter(|p| !p.trim().is_empty())
                .map(|p| resolve_file_path(workspace_root, p));
            let candidates = if is_multi_candidate_navigation_tool(tool_name) {
                let max_candidates = args
                    .get("max_candidates")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize)
                    .unwrap_or(DEFAULT_AMBIGUOUS_CANDIDATE_LIMIT);
                Box::pin(resolve_symbol_candidates(
                    remote,
                    workspace_root,
                    &symbol_trimmed,
                    hint.as_deref(),
                    max_candidates,
                ))
                .await?
            } else {
                vec![
                    Box::pin(resolve_symbol(
                        remote,
                        workspace_root,
                        &symbol_trimmed,
                        hint.as_deref(),
                    ))
                    .await?,
                ]
            };

            if candidates.len() > 1 {
                if tool_name == "code_definition" {
                    return Ok(McpToolCallResult::text(
                        super::symbols::matching::ambiguous_candidates_error(
                            workspace_root,
                            &symbol_trimmed,
                            &candidates,
                        )
                        .to_string(),
                    ));
                }
                let total = candidates.len();
                let shared_budget = if let Some(ms) = args.get("budget_ms").and_then(|v| v.as_u64())
                {
                    std::time::Duration::from_millis(ms.max(1))
                } else if let Some(secs) = args
                    .get("budget_secs")
                    .or_else(|| args.get("timeout_secs"))
                    .and_then(|v| v.as_u64())
                {
                    std::time::Duration::from_secs(secs.clamp(1, 90))
                } else {
                    crate::call_tree::DEFAULT_CALL_TREE_BUDGET
                };
                let deadline = std::time::Instant::now() + shared_budget;
                let mut out = format!("`{symbol_trimmed}` matched {total} candidates:\n");
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
                    let mut cand_args = args.clone();
                    if let Some(obj) = cand_args.as_object_mut() {
                        obj.insert(
                            "path".into(),
                            serde_json::Value::String(hit.path.to_string_lossy().into_owned()),
                        );
                        obj.insert("line".into(), serde_json::json!(hit.line));
                        obj.insert("character".into(), serde_json::json!(hit.col));
                        obj.insert(
                            "budget_ms".into(),
                            serde_json::json!((remaining.as_millis() as u64).max(1)),
                        );
                    }
                    validate_position_arguments(tool_name, &cand_args)?;
                    let cand_remote = crate::cluster::route_for_path(
                        remote,
                        workspace_root,
                        cand_args.get("path").and_then(|v| v.as_str()),
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
                            cand_remote,
                            workspace_root,
                            tool_name,
                            cand_args,
                        )),
                    )
                    .await
                    {
                        Ok(Ok(res)) => {
                            if !res.is_error {
                                any_ok = true;
                            }
                            for item in &res.content {
                                let crate::protocol::McpContentItem::Text { text } = item;
                                out.push_str(text.trim_end());
                                out.push('\n');
                            }
                        }
                        Ok(Err(err)) => {
                            out.push_str(&format!("Error: {err:#}\n"));
                        }
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
                let combined = if any_ok {
                    Ok(McpToolCallResult::text(out.trim_end().to_string()))
                } else {
                    Ok(McpToolCallResult::error(out.trim_end().to_string()))
                };
                return with_indexing_notes(combined, workspace_root);
            }

            let hit = &candidates[0];
            let mut owned = args.clone();
            if let Some(obj) = owned.as_object_mut() {
                obj.insert(
                    "path".into(),
                    serde_json::Value::String(hit.path.to_string_lossy().into_owned()),
                );
                obj.insert("line".into(), serde_json::json!(hit.line));
                obj.insert("character".into(), serde_json::json!(hit.col));
            }
            owned
        } else {
            args
        };
        // Resolved symbol positions obey the same contract, including a supplied selection end.
        validate_position_arguments(tool_name, &args)?;
        // A path in a nested project of another language goes to a node that serves it (#125).
        let remote = crate::cluster::route_for_path(
            remote,
            workspace_root,
            routed_path(tool_name, args.get("path").and_then(|v| v.as_str())),
        )
        .await?;
        let result = Box::pin(dispatch_tool(remote, workspace_root, tool_name, args)).await;
        with_indexing_notes(result, workspace_root)
    })
}
