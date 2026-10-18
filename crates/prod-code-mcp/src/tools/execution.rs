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
use super::list_tools;
use super::references::references_across;
use super::schemas::{POSITION_ARGUMENTS, SYMBOL_ADDRESSABLE};
use super::symbols::resolve_symbol;
use crate::protocol::McpToolCallResult;

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

/// Check only position properties the tool actually advertises. Keeping this registry derived
/// from discovery means a newly exposed position tool gets the same input contract immediately.
fn validate_position_arguments(tool_name: &str, args: &serde_json::Value) -> Result<()> {
    static POSITION_TOOLS: std::sync::OnceLock<Vec<(String, Vec<&'static str>)>> =
        std::sync::OnceLock::new();
    let positions = POSITION_TOOLS.get_or_init(|| {
        list_tools()
            .into_iter()
            .filter_map(|tool| {
                let properties = tool.input_schema.get("properties")?.as_object()?;
                let fields: Vec<_> = POSITION_ARGUMENTS
                    .into_iter()
                    .filter(|name| properties.contains_key(*name))
                    .collect();
                (!fields.is_empty()).then_some((tool.name, fields))
            })
            .collect()
    });
    let Some((_, fields)) = positions.iter().find(|(name, _)| name == tool_name) else {
        return Ok(());
    };
    for name in fields {
        if let Some(value) = args.get(*name) {
            checked_position_argument(value, name)?;
        }
    }
    if fields.contains(&"end_line") && fields.contains(&"end_character") {
        let end = match (args.get("end_line"), args.get("end_character")) {
            (None, None) => return Ok(()),
            (Some(line), Some(character)) => (
                checked_position_argument(line, "end_line")?,
                checked_position_argument(character, "end_character")?,
            ),
            _ => anyhow::bail!("a selection requires both 'end_line' and 'end_character'"),
        };
        if let (Some(line), Some(character)) = (args.get("line"), args.get("character")) {
            let start = (
                checked_position_argument(line, "line")?,
                checked_position_argument(character, "character")?,
            );
            anyhow::ensure!(end >= start, "selection end precedes its start");
        }
    }
    Ok(())
}

static WORKSPACE_MUTATION_LOCKS: std::sync::LazyLock<
    std::sync::Mutex<
        std::collections::HashMap<std::path::PathBuf, std::sync::Arc<tokio::sync::RwLock<()>>>,
    >,
> = std::sync::LazyLock::new(Default::default);

pub(crate) fn workspace_mutation_lock(
    workspace_root: &Path,
) -> std::sync::Arc<tokio::sync::RwLock<()>> {
    let key =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    WORKSPACE_MUTATION_LOCKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key)
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::RwLock::new(())))
        .clone()
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
        | "code_slice"
        | "code_impact"
        | "code_diagnostics"
        | "code_validate_edit"
        | "code_validate_edits"
        | "code_shadow_run"
        | "code_assists"
        | "code_dead_code"
        | "code_dependencies"
        | "code_find_duplicates"
        | "code_structural_search"
        | "code_propose_expression"
        | "code_status"
        | "code_report_issue"
        | "code_check"
        | "code_lint"
        | "code_test"
        | "code_benchmarks"
        | "code_diagnose_failure" => true,
        "code_exec" => args.get("pull_changes").and_then(|v| v.as_bool()) != Some(true),
        _ => false,
    }
}

enum WorkspaceMutationGuard {
    Read(#[allow(dead_code)] tokio::sync::OwnedRwLockReadGuard<()>),
    Write(#[allow(dead_code)] tokio::sync::OwnedRwLockWriteGuard<()>),
}

/// Execute an MCP tool call against the remote gateway.
pub async fn execute_tool(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: serde_json::Value,
) -> Result<McpToolCallResult> {
    // Invalid explicit positions are refused before symbol lookup, routing or any write.
    validate_position_arguments(tool_name, &args)?;
    // Notes left by an earlier call are not this answer's (#391).
    let _ = crate::session::take_indexing_notes(workspace_root);
    if tool_name == "code_references"
        && let Some(dirs) = args.get("also_in").and_then(|v| v.as_array())
        && !dirs.is_empty()
    {
        let dirs = dirs.clone();
        return references_across(remote, workspace_root, args, &dirs).await;
    }
    let lock = workspace_mutation_lock(workspace_root);
    let _guard = if is_read_only_tool(tool_name, &args) {
        WorkspaceMutationGuard::Read(lock.read_owned().await)
    } else {
        WorkspaceMutationGuard::Write(lock.write_owned().await)
    };
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
    let remote =
        crate::cluster::route_for_path(remote, workspace_root, initial_path.as_deref()).await?;

    // `symbol` instead of line/character: resolve the name through the workspace symbol
    // index, then run the tool at that position.
    let args = if SYMBOL_ADDRESSABLE.contains(&tool_name)
        && let Some(symbol) = args.get("symbol").and_then(|v| v.as_str())
        && !symbol.trim().is_empty()
    {
        let hint = args
            .get("path")
            .and_then(|v| v.as_str())
            .filter(|p| !p.trim().is_empty())
            .map(|p| resolve_file_path(workspace_root, p));
        let hit = Box::pin(resolve_symbol(
            remote,
            workspace_root,
            symbol.trim(),
            hint.as_deref(),
        ))
        .await?;
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
        args.get("path").and_then(|v| v.as_str()),
    )
    .await?;
    let result = Box::pin(dispatch_tool(remote, workspace_root, tool_name, args)).await;
    with_indexing_notes(result, workspace_root)
}
