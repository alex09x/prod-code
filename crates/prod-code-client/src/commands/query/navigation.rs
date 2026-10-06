/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::commands::common::{position, run_tool, symbol_args};
use crate::workspace::find_workspace_root;
use anyhow::{Context, Result};
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Runs `code_references` tool from the current checkout and prints references; exit 1 on error or no references.
pub async fn run_refs(remote: SocketAddr, args: serde_json::Value) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut remote = remote;
    let mut result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_references", args.clone()).await;
    if let Err(ref e) = result
        && prod_code_mcp::is_retryable_connection_error("code_references", e)
        && let Some(new_addr) = prod_code_mcp::rediscover_node(remote, &root).await
    {
        remote = new_addr;
        let identity = prod_code_mcp::sync::workspace_identity(&root);
        let name = identity.base.unwrap_or(identity.name);
        prod_code_mcp::cluster::remember_placement(&name, new_addr);
        result = prod_code_mcp::tools::execute_tool(remote, &root, "code_references", args).await;
    }
    let result = result?;
    let mut has_refs = false;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        if text.contains("Found ") && text.contains(" reference(s)") {
            has_refs = true;
        }
        println!("{text}");
    }
    if result.is_error || !has_refs {
        std::process::exit(1);
    }
    Ok(())
}

pub async fn run_call_tree(
    remote: SocketAddr,
    tool: &str,
    file: Option<PathBuf>,
    line: Option<u32>,
    col: Option<u32>,
    symbol: Option<String>,
    depth: usize,
) -> Result<()> {
    let args = match symbol {
        Some(symbol) => {
            let mut args = symbol_args(&symbol, file);
            args["depth"] = serde_json::json!(depth);
            args
        }
        None => {
            let (file, line, col) = position(file, line, col)?;
            let file = std::fs::canonicalize(&file).unwrap_or(file);
            serde_json::json!({
                "path": file.to_string_lossy(),
                "line": line,
                "character": col,
                "depth": depth,
            })
        }
    };
    run_tool(remote, tool, args).await
}

pub async fn run_symbols(
    remote: SocketAddr,
    file: &Path,
    options: &prod_code_mcp::tools::OutlineOptions,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    if abs_path.is_dir() {
        let cwd = env::current_dir().context("Failed to determine current working directory")?;
        let ws_root = find_workspace_root(&abs_path).unwrap_or_else(|| cwd.clone());
        let text =
            prod_code_mcp::tools::outline_directory(remote, &ws_root, &abs_path, file, options)
                .await?;
        let has_symbols = text.lines().any(|l| l.trim_start().starts_with('['))
            || text.contains("subdirectories with sources:");
        if !has_symbols {
            eprintln!("no outline symbols found for {}", file.display());
            std::process::exit(1);
        }
        println!("{text}");
        return Ok(());
    }

    let cwd = env::current_dir().context("Failed to determine current working directory")?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let text = prod_code_mcp::tools::outline_file(
        remote,
        &ws_root,
        &abs_path,
        &file.display().to_string(),
        options,
    )
    .await?;
    let has_symbols = text.lines().any(|l| l.trim_start().starts_with('['));
    if !has_symbols {
        eprintln!("no outline symbols found for {}", file.display());
        std::process::exit(1);
    }
    println!("{text}");
    Ok(())
}

pub async fn run_source(
    remote: SocketAddr,
    root: Option<&Path>,
    path: &str,
    line: Option<u32>,
    context: u32,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone()));
    let (bytes, truncated) = prod_code_mcp::remote_fs::read_source(remote, &root, path).await?;
    let text = String::from_utf8_lossy(&bytes);
    match line {
        Some(line) => print!(
            "{}",
            prod_code_mcp::remote_fs::snippet(&text, line, context)
        ),
        None => print!("{text}"),
    }
    if truncated {
        eprintln!("[prod-code] {path}: output truncated at 2 MiB");
    }
    Ok(())
}

pub async fn run_search_cli(
    remote: SocketAddr,
    query: String,
    limit: usize,
    path: Option<String>,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let subpath = path.or_else(|| prod_code_mcp::exec::subdir_of(&root, &cwd));
    let resp =
        prod_code_mcp::search::search(remote, &root, &query, limit, subpath.as_deref()).await?;
    println!("{}", prod_code_mcp::search::render(&resp, &query));
    Ok(())
}

pub async fn run_slice(
    remote: SocketAddr,
    target: String,
    line: Option<u32>,
    character: u32,
    mut options: prod_code_mcp::slice::SliceOptions,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let cursor_line = line;
    let (file, line, col) = match line {
        Some(line) => {
            let path = PathBuf::from(&target);
            let abs = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            (std::fs::canonicalize(&abs).unwrap_or(abs), line, character)
        }
        None => {
            let hit = prod_code_mcp::tools::resolve_symbol(remote, &root, &target, None).await?;
            println!(
                "{} {} at {}:{}:{}",
                hit.kind,
                hit.name,
                hit.path
                    .strip_prefix(&root)
                    .unwrap_or(&hit.path)
                    .to_string_lossy(),
                hit.line,
                hit.col
            );
            (hit.path, hit.line, hit.col)
        }
    };
    if options.dataflow
        && options.target_line.is_none()
        && let Some(explicit_line) = cursor_line
    {
        options.target_line = Some(explicit_line);
    }
    let report =
        prod_code_mcp::slice::slice_with_options(remote, &root, &file, line, col, options).await?;
    let rendered = report.render();
    if report.items.is_empty() {
        eprintln!("no slice items found for {target}");
        std::process::exit(1);
    }
    println!("{rendered}");
    Ok(())
}
