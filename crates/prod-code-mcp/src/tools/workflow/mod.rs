/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Bounded composition of existing investigation operations. No source mutation or shell.
mod metadata;
#[cfg(test)]
mod metadata_tests;
mod plan;
mod references;
mod runner;
mod schema;
mod search;
mod short_form;
#[cfg(test)]
mod tests;

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
    let mut selection = json!({});
    if let Some(workspace) = &plan.workspace {
        selection["workspace"] = json!(workspace);
    }
    // Reuse the existing selector and confinement contract; the workflow has one root.
    let root = resolve_workspace_root(configured, "code_definition", &mut selection)?;
    let report = runner::run(plan, |tool, args| {
        let root = root.clone();
        async move { invoke(remote, &root, &tool, args).await }
    })
    .await;
    let mut result = McpToolCallResult::text(serde_json::to_string(&report)?);
    result.structured_content = Some(report);
    Ok(result)
}

async fn invoke(remote: SocketAddr, root: &Path, tool: &str, mut args: Value) -> Result<Value> {
    // Always confine explicitly, including when the workflow selected its default root.
    args["workspace"] = json!(root);
    resolve_workspace_root(root, "code_definition", &mut args)?;
    args.as_object_mut()
        .context("args must be an object")?
        .remove("workspace");
    let _read_guard = if matches!(tool, "resolve" | "symbols" | "grep") {
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
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(8)
                .clamp(1, 8) as usize;
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
    let path = hit
        .path
        .strip_prefix(root)
        .context("symbol candidate is outside the workflow workspace")?;
    Ok(
        json!({"path":path,"line":hit.line,"character":hit.col,"name":hit.name,"kind":hit.kind,"container":hit.container}),
    )
}
