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
use std::path::PathBuf;

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

/// Runs one MCP tool from the current checkout and prints what it says; exit 1 on an error.
pub async fn run_tool(remote: SocketAddr, tool: &str, args: serde_json::Value) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut remote = remote;
    let mut result = prod_code_mcp::tools::execute_tool(remote, &root, tool, args.clone()).await;
    if let Err(ref e) = result
        && prod_code_mcp::is_retryable_connection_error(tool, e)
        && let Some(new_addr) = prod_code_mcp::rediscover_node(remote, &root).await
    {
        remote = new_addr;
        let identity = prod_code_mcp::sync::workspace_identity(&root);
        let name = identity.base.unwrap_or(identity.name);
        prod_code_mcp::cluster::remember_placement(&name, new_addr);
        result = prod_code_mcp::tools::execute_tool(remote, &root, tool, args).await;
    }
    let result = result?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
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
