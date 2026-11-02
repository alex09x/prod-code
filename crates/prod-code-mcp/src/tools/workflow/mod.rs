/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Bounded composition of investigation, guarded text edits and explicit verification.
mod edits;
mod metadata;
#[cfg(test)]
mod metadata_tests;
#[cfg(test)]
mod mutation_api_tests;
#[cfg(test)]
mod mutation_plan_tests;
#[cfg(test)]
mod mutation_review_tests;
mod mutations;
mod operation_failure;
mod plan;
mod references;
#[cfg(test)]
mod review_regressions;
mod runner;
mod schema;
mod search;
mod short_form;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
mod verification;
#[cfg(test)]
mod verification_tests;

use super::{execute_tool, execution::resolve_workspace_root, symbols};
use crate::protocol::{McpContentItem, McpToolCallResult};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{net::SocketAddr, path::Path};

pub(super) fn tool_schema() -> crate::protocol::McpTool {
    schema::tool()
}

pub(super) async fn execute(
    remote: SocketAddr,
    configured: &Path,
    args: Value,
) -> Result<McpToolCallResult> {
    let plan = plan::Plan::parse(short_form::expand(args)?)?;
    let structured = plan.structured;
    let mut selection = json!({});
    if let Some(workspace) = &plan.workspace {
        selection["workspace"] = json!(workspace);
    }
    // Reuse the existing selector and confinement contract; the workflow has one root.
    let root = resolve_workspace_root(configured, "code_definition", &mut selection)?;
    verification::preflight(&plan, &root)?;
    let report = runner::run(plan, |tool, args| {
        let root = root.clone();
        async move { invoke(remote, &root, &tool, args).await }
    })
    .await;
    let mut result = McpToolCallResult::text(serde_json::to_string(&report)?);
    if structured {
        result.structured_content = Some(report);
    }
    Ok(result)
}

async fn invoke(remote: SocketAddr, root: &Path, tool: &str, mut args: Value) -> Result<Value> {
    prepare_arguments(root, tool, &mut args)
        .map_err(|error| operation_failure::before(tool, error))?;
    invoke_prepared(remote, root, tool, args).await
}

fn prepare_arguments(root: &Path, tool: &str, args: &mut Value) -> Result<()> {
    plan::validate_adapter_arguments(tool, args)?;
    if args
        .get("path")
        .and_then(Value::as_str)
        .is_some_and(|path| path.trim().is_empty())
    {
        args.as_object_mut()
            .context("args must be an object")?
            .remove("path");
    }
    // Always confine explicitly, including when the workflow selected its default root.
    args["workspace"] = json!(root);
    resolve_workspace_root(root, "code_definition", args)?;
    args.as_object_mut()
        .context("args must be an object")?
        .remove("workspace");
    Ok(())
}

async fn invoke_prepared(
    remote: SocketAddr,
    root: &Path,
    tool: &str,
    args: Value,
) -> Result<Value> {
    let _read_guard = if matches!(tool, "resolve" | "symbols" | "grep" | "preview") {
        let _ = crate::session::take_indexing_notes(root);
        Some(
            super::execution::workspace_mutation_lock(root)
                .read_owned()
                .await,
        )
    } else {
        None
    };
    let mut outcome = match tool {
        "preview" | "apply" | "verify" => mutations::invoke(remote, root, tool, args).await,
        "grep" => search::grep(remote, root, args).await,
        "resolve" => {
            let symbol = args
                .get("symbol")
                .and_then(Value::as_str)
                .context("resolve requires symbol")?;
            let hint = args
                .get("path")
                .and_then(Value::as_str)
                .map(|path| root.join(path));
            let hit = symbols::resolve_symbol(remote, root, symbol, hint.as_deref()).await?;
            hit_value(root, hit)
        }
        "symbols" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .context("symbols requires query")?;
            let limit = plan::limit(&args, 8)?;
            let hint = args
                .get("path")
                .and_then(Value::as_str)
                .map(|path| root.join(path));
            let hits = symbols::symbol_search_across_projects(
                remote,
                root,
                query,
                hint.as_deref(),
                limit + 1,
            )
            .await?;
            let truncated = hits.len() > limit;
            let items = hits
                .into_iter()
                .take(limit)
                .map(|hit| hit_value(root, hit))
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({"items":items,"truncated":truncated,"exhaustive":false}))
        }
        _ => {
            let result = execute_tool(remote, root, tool, args).await?;
            let mut data = result
                .structured_content
                .as_ref()
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let text = result
                .content
                .into_iter()
                .map(|content| match content {
                    McpContentItem::Text { text } => text,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if result.is_error {
                bail!("{text}");
            }
            data.insert("text".into(), json!(text));
            Ok(Value::Object(data))
        }
    };
    if matches!(tool, "resolve" | "symbols" | "grep") {
        let notes = crate::session::take_indexing_notes(root);
        if !notes.is_empty()
            && let Ok(value) = &mut outcome
        {
            value["indexing_notes"] = json!(notes);
        }
    }
    outcome
}

fn hit_value(root: &Path, hit: symbols::SymbolHit) -> Result<Value> {
    let normalized = std::fs::canonicalize(&hit.path).unwrap_or_else(|_| hit.path.clone());
    let path = normalized
        .strip_prefix(root)
        .context("symbol candidate is outside the workflow workspace")?;
    let path = path.to_str().context("symbol path is not valid UTF-8")?;
    Ok(
        json!({"path":path,"line":hit.line,"character":hit.col,"name":hit.name,"kind":hit.kind,"container":hit.container}),
    )
}
