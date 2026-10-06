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
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use std::net::SocketAddr;
use std::path::Path;
use tokio_util::codec::Framed;
use url::Url;

use super::{execute_lsp_query, resolve_file_path};
use crate::protocol::McpToolCallResult;
use crate::sync::{push_workspace_sync, workspace_identity};

pub(crate) async fn handle_dependencies(
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let scope_str = args
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("crates");
    let scope = match scope_str {
        "modules" => crate::dependencies::DependencyScope::Modules,
        _ => crate::dependencies::DependencyScope::Crates,
    };
    let target_path = args.get("path").and_then(|v| v.as_str()).map(Path::new);

    let report = crate::dependencies::analyze_dependencies(workspace_root, scope, target_path)?;
    let output = crate::dependencies::format_dependency_report(&report);
    Ok(McpToolCallResult::text(output))
}

pub(crate) async fn handle_find_duplicates(
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let min_lines = args.get("min_lines").and_then(|v| v.as_u64()).unwrap_or(6) as usize;
    let parameterized = args
        .get("parameterized")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let type3 = args.get("type3").and_then(|v| v.as_bool()).unwrap_or(false);
    let max_groups = args
        .get("max_groups")
        .and_then(|v| v.as_u64())
        .unwrap_or(20) as usize;
    let target_path = args.get("path").and_then(|v| v.as_str()).map(Path::new);

    let options = crate::duplicates::DuplicateOptions {
        min_lines,
        parameterized,
        type3,
        max_groups,
    };

    let report = crate::duplicates::find_duplicates(workspace_root, target_path, options)?;
    let output = crate::duplicates::format_duplication_report(&report);
    Ok(McpToolCallResult::text(output))
}

pub(crate) async fn handle_structural_search(
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .context("Missing 'pattern' argument")?;
    let scope = args.get("path").and_then(|v| v.as_str()).map(Path::new);

    let result = crate::codemod::run_structural_search(workspace_root, pattern, scope)?;
    let output = result.render(25);
    Ok(McpToolCallResult::text(output))
}

pub(crate) async fn handle_propose_expression(
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let target_type = args
        .get("target_type")
        .and_then(|v| v.as_str())
        .context("Missing 'target_type' argument")?;

    let report = crate::expression_synthesis::propose_expressions_in_scope(
        workspace_root,
        path,
        line,
        target_type,
    )?;
    let output = crate::expression_synthesis::format_expression_synthesis_report(&report);
    Ok(McpToolCallResult::text(output))
}

/// A tool's answer with a note for every index question the language server answered while it
/// was still loading or indexing: it may be incomplete, and says so instead of passing for the
/// whole answer (#391).
pub(crate) fn with_indexing_notes(
    result: Result<McpToolCallResult>,
    root: &Path,
) -> Result<McpToolCallResult> {
    let notes = crate::session::take_indexing_notes(root);
    let mut result = result?;
    for note in notes {
        result.content.push(crate::protocol::McpContentItem::Text {
            text: format!(
                "(the language server was still {note} when asked: this answer may be incomplete)"
            ),
        });
    }
    Ok(result)
}

pub(crate) async fn handle_sync(
    remote: SocketAddr,
    workspace_root: &Path,
    args: serde_json::Value,
) -> Result<McpToolCallResult> {
    let subpath = args.get("path").and_then(|v| v.as_str()).map(Path::new);
    let identity = workspace_identity(workspace_root);
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let outcome = push_workspace_sync(&mut framed, workspace_root, &identity, subpath).await?;
    let kb = (outcome.bytes_transferred as f64) / 1024.0;
    let remote_root = if outcome.server_workspace_root.is_empty() {
        "(not reported)"
    } else {
        &outcome.server_workspace_root
    };
    let info = format!(
        "⚡ Fast-Sync Completed\n\
                 • Files planned: {}\n\
                 • Files updated: {}\n\
                 • Files deleted: {}\n\
                 • Data transferred: {kb:.1} KB\n\
                 • Remote workspace: {remote_root}",
        outcome.planned, outcome.files_updated, outcome.files_deleted
    );
    Ok(McpToolCallResult::text(info))
}

pub(crate) async fn handle_status(remote: SocketAddr) -> Result<McpToolCallResult> {
    let start = std::time::Instant::now();
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to gateway at {remote}"))?;
    let rtt = start.elapsed();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed.send(WireMessage::StatusRequest).await?;
    if let Some(msg_res) = framed.next().await {
        match msg_res? {
            WireMessage::StatusResponse(resp) => {
                let hours = resp.uptime_seconds / 3600;
                let minutes = (resp.uptime_seconds % 3600) / 60;
                let seconds = resp.uptime_seconds % 60;
                let mem = resp.memory_rss_mb().unwrap_or(0.0);
                let host = match resp.host.describe() {
                    described if described.is_empty() => String::new(),
                    described => format!("\n• Host: {described}"),
                };
                let health = match resp.host.pressure() {
                    Some(why) => format!("SHORT ({why}): new workspaces go to other nodes"),
                    None => "HEALTHY".to_string(),
                };

                let info = format!(
                    "⚡ prod-code Gateway Status\n\
                             • Address: {remote} ({rtt:.2?} RTT)\n\
                             • Server PID: {}\n\
                             • Uptime: {hours}h {minutes}m {seconds}s\n\
                             • Memory RSS: {mem:.2} MB{host}\n\
                             • Active Sessions: {}\n\
                             • Running Commands: {}{}\n\
                             • Loaded Workspaces: {}\n\
                             • Queries Handled: {} (in-flight: {})\n\
                             • Engines: {}\n\
                             • Status: {health}",
                    resp.server_pid,
                    resp.active_sessions,
                    resp.running_commands.len(),
                    resp.running_lines()
                        .iter()
                        .map(|line| format!("\n    - {line}"))
                        .collect::<String>(),
                    resp.loaded_workspaces,
                    resp.total_queries,
                    resp.active_queries,
                    resp.detected_engines.join(", ")
                );
                Ok(McpToolCallResult::text(info))
            }
            other => Ok(McpToolCallResult::error(format!(
                "Unexpected response from gateway: {other:?}"
            ))),
        }
    } else {
        Ok(McpToolCallResult::error(
            "Gateway closed connection without status response",
        ))
    }
}

pub(crate) async fn handle_hover(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/hover",
        params,
    )
    .await?;
    if let Some(contents) = res.get("contents") {
        if let Some(val) = contents.get("value").and_then(|v| v.as_str()) {
            return Ok(McpToolCallResult::text(val));
        } else if let Some(arr) = contents.as_array() {
            let text = arr
                .iter()
                .filter_map(|i| i.get("value").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join("\n\n");
            return Ok(McpToolCallResult::text(text));
        }
    }
    Ok(McpToolCallResult::text("No hover information available."))
}
