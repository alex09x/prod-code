/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::workspace::find_workspace_root;
use anyhow::{Context, Result};
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "common_tests.rs"]
mod coverage_tests;
#[cfg(test)]
#[path = "common_pin_tests.rs"]
pub(crate) mod pin_tests;

/// The position a command was given, when it was not given `--symbol`.
pub fn position(
    file: Option<PathBuf>,
    line: Option<u32>,
    col: Option<u32>,
) -> Result<(PathBuf, u32, u32)> {
    match (file, line, col) {
        (Some(file), Some(line), Some(col)) => Ok((file, line, col)),
        _ => anyhow::bail!("give <file> <line> <col>, or --symbol NAME"),
    }
}

/// The MCP arguments of `--symbol NAME [FILE]`.
pub fn symbol_args(symbol: &str, file: Option<PathBuf>) -> serde_json::Value {
    let mut args = serde_json::json!({ "symbol": symbol });
    if let Some(file) = file {
        let file = std::fs::canonicalize(&file).unwrap_or(file);
        args["path"] = serde_json::json!(file.to_string_lossy());
    }
    args
}

/// The MCP arguments of one or more repeatable `--symbol NAME` flags (#1123).
pub fn symbols_args(symbols: &[String], file: Option<PathBuf>) -> serde_json::Value {
    let mut args = if symbols.len() == 1 {
        serde_json::json!({ "symbol": symbols[0] })
    } else {
        serde_json::json!({ "symbols": symbols })
    };
    if let Some(file) = file {
        let file = std::fs::canonicalize(&file).unwrap_or(file);
        args["path"] = serde_json::json!(file.to_string_lossy());
    }
    args
}

/// Runs one MCP tool from the current checkout and prints what it says; exit 1 on an error.
pub async fn run_tool(remote: SocketAddr, tool: &str, args: serde_json::Value) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    run_tool_in(remote, &root, tool, args).await
}

/// [`run_tool`] for the checkout at `root`, which a command naming a file of another checkout
/// takes from that file, not from the current directory (#1455).
pub async fn run_tool_in(
    remote: SocketAddr,
    root: &Path,
    tool: &str,
    args: serde_json::Value,
) -> Result<()> {
    let result = run_tool_result_in(remote, root, tool, args).await?;
    if tool == "code_definition"
        && result.content.len() == 1
        && matches!(&result.content[0], prod_code_mcp::protocol::McpContentItem::Text { text }
            if text.trim() == "No definition found.")
    {
        anyhow::bail!("no definition found");
    }
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

/// Executes one CLI tool under its selected checkout's policy, including retries.
pub(super) async fn run_tool_result_in(
    remote: SocketAddr,
    root: &Path,
    tool: &str,
    args: serde_json::Value,
) -> Result<prod_code_mcp::protocol::McpToolCallResult> {
    run_tool_in_with(remote, root, tool, args, |old, root| async move {
        prod_code_mcp::rediscover_node(old, &root).await
    })
    .await
}

async fn run_tool_in_with<D, F>(
    remote: SocketAddr,
    root: &Path,
    tool: &str,
    args: serde_json::Value,
    rediscover: D,
) -> Result<prod_code_mcp::protocol::McpToolCallResult>
where
    D: Fn(SocketAddr, PathBuf) -> F,
    F: std::future::Future<Output = Option<SocketAddr>>,
{
    let timeout = prod_code_mcp::resolve_tool_call_timeout(&args);
    let deadline = tokio::time::Instant::now() + timeout;
    let mut args = args;
    let selector = if tool == "code_workflow" {
        "code_definition"
    } else {
        tool
    };
    let root = prod_code_mcp::tools::resolve_workspace_root(root, selector, &mut args)?;
    let policy = prod_code_mcp::cluster::policy_for_root(&root)?;
    let remote = prod_code_mcp::cluster::place_within(&policy, remote)?;
    prod_code_mcp::cluster::with_root_policy(&root, policy.clone(), async {
        let mut remote = remote;
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "{tool} timed out after {}s; pass a file path hint or check gateway connectivity",
                timeout.as_secs()
            );
        }
        let mut result = tokio::time::timeout_at(
            deadline,
            prod_code_mcp::tools::execute_tool(remote, &root, tool, args.clone()),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "{tool} timed out after {}s; pass a file path hint or check gateway connectivity",
                timeout.as_secs()
            )
        })?;
        // A pinned workspace retries only a call that never started, and only within its pin; any
        // move is reported (#1793).
        if let Err(ref e) = result
            && prod_code_mcp::server::retry_permitted(&policy, tool, e)
            && tokio::time::Instant::now() < deadline
            && (!policy.is_pinned() || !policy.alternatives(remote).is_empty())
            && let Ok(Some(new_addr)) =
                tokio::time::timeout_at(deadline, rediscover(remote, root.clone())).await
            && policy.allows(new_addr)
        {
            let note = prod_code_mcp::server::failover_pin::relocation_note(remote, new_addr, e);
            remote = new_addr;
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "{tool} timed out after {}s on failover node; check gateway connectivity",
                    timeout.as_secs()
                );
            }
            result = tokio::time::timeout_at(
                deadline,
                prod_code_mcp::tools::execute_tool(remote, &root, tool, args),
            )
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "{tool} timed out after {}s on failover node; check gateway connectivity",
                    timeout.as_secs()
                )
            })?;
            if result.is_ok() {
                if !policy.is_pinned() {
                    let identity = prod_code_mcp::sync::workspace_identity(&root);
                    let name = identity.base.unwrap_or(identity.name);
                    prod_code_mcp::cluster::remember_placement(&name, new_addr);
                }
                eprintln!("{note}");
            }
        }
        // The MCP navigation timer may return its busy result in the same tick as the
        // outer timer. Keep CLI navigation timeouts errors, not process exits.
        if matches!(tool, "code_references" | "code_definition")
            && tokio::time::Instant::now() >= deadline
            && result.as_ref().is_ok_and(|reply| reply.is_error)
        {
            anyhow::bail!(
                "{tool} timed out after {}s; pass a file path hint or check gateway connectivity",
                timeout.as_secs()
            );
        }
        result
    })
    .await
}

/// A position command given `--symbol`: the MCP tool resolves the name, exactly as for an agent,
/// among the candidates in `file` when one is given.
pub async fn run_by_symbol(
    remote: SocketAddr,
    tool: &str,
    symbol: &str,
    file: Option<PathBuf>,
) -> Result<()> {
    run_tool(remote, tool, symbol_args(symbol, file)).await
}

/// A position command given one or more `--symbol` flags (#1123).
pub async fn run_by_symbols(
    remote: SocketAddr,
    tool: &str,
    symbols: &[String],
    file: Option<PathBuf>,
) -> Result<()> {
    run_tool(remote, tool, symbols_args(symbols, file)).await
}

/// Parses a `LINE:COL` string into 1-based (line, col) tuple.
pub fn parse_line_col(spec: &str) -> Result<(u32, u32)> {
    let (l, c) = spec
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("expected LINE:COL, got {spec}"))?;
    Ok((l.trim().parse()?, c.trim().parse()?))
}

/// Prints `uri:line:col` for every LSP `Location` in `arr`.
pub fn print_locations(arr: &[serde_json::Value]) {
    for loc in arr {
        let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let start = loc.get("range").and_then(|r| r.get("start"));
        let line = start
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0)
            + 1;
        let col = start
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0)
            + 1;
        println!("  • {uri}:{line}:{col}");
    }
}

/// Truncates string to at most `n` characters, appending ellipsis if truncated.
pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_tool_times_out_with_actionable_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // A gateway that accepts and then says nothing. The connection is held for the whole
        // wait: dropped at once (`let _ = …`), it was reset under the client, which then
        // sometimes reported the reset before its own timeout.
        tokio::spawn(async move {
            let _silent = listener.accept().await;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        });

        let err = run_tool(
            addr,
            "code_definition",
            serde_json::json!({
                "symbol": "NonExistent",
                "timeout_millis": 50
            }),
        )
        .await
        .expect_err("must time out");

        let msg = format!("{err:#}");
        assert!(msg.contains("timed out"), "{msg}");
        assert!(msg.contains("check gateway connectivity"), "{msg}");
    }
}
