/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod ast;
pub mod schema;

pub use ast::*;
pub use schema::*;

use crate::commands::common::{parse_line_col, run_tool};
use crate::commands::query::execute_lsp_query;
use crate::commands::exec::verify_scope;
use crate::workspace::find_workspace_root;
use anyhow::{Context, Result};
use prod_code_mcp::verify::VerifyKind;
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use url::Url;

/// Delete an unreferenced item through the remote analyzer and apply the edit locally.
pub async fn run_safe_delete(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    run_tool(
        remote,
        "code_safe_delete",
        serde_json::json!({
            "path": abs_path.to_string_lossy(),
            "line": line,
            "character": col,
        }),
    )
    .await
}

/// List code actions at a position (no `id`) or apply one (`id`) and write its edits locally.
pub async fn run_assist(
    remote: SocketAddr,
    file: &Path,
    line: u32,
    col: u32,
    to: Option<&str>,
    id: Option<&str>,
    subtype: Option<u64>,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();
    let end = match to {
        Some(spec) => parse_line_col(spec)?,
        None => (line, col),
    };
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "range": {
            "start": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            "end": { "line": end.0.saturating_sub(1), "character": end.1.saturating_sub(1) }
        }
    });
    match id {
        None => {
            let list = execute_lsp_query(remote, file, "prodCode/assists", params).await?;
            let items = list.as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                println!("no code actions at {}:{line}:{col}", file.display());
            }
            for item in items {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
                match item.get("subtype").and_then(|v| v.as_u64()) {
                    Some(st) => println!("{id} --subtype {st}  [{kind}]  {label}"),
                    None => println!("{id}  [{kind}]  {label}"),
                }
            }
            Ok(())
        }
        Some(id) => {
            let started = std::time::Instant::now();
            let mut args = serde_json::json!({
                "path": abs_path.to_string_lossy(),
                "line": line,
                "character": col,
                "end_line": end.0,
                "end_character": end.1,
                "id": id,
            });
            if let Some(st) = subtype {
                args["subtype"] = serde_json::json!(st);
            }
            let result =
                prod_code_mcp::tools::execute_tool(remote, &ws_root, "code_assist", args).await?;
            for content in &result.content {
                let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
                println!("{text}");
            }
            if result.is_error {
                std::process::exit(1);
            }
            println!("[{:.2}s]", started.elapsed().as_secs_f64());
            Ok(())
        }
    }
}

/// Rename a symbol through the remote analyzer and apply the resulting edits to the checkout.
#[allow(clippy::too_many_arguments)]
pub async fn run_rename(
    remote: SocketAddr,
    file: &Path,
    line: u32,
    col: u32,
    new_name: &str,
    accessors: bool,
    comments: bool,
    force: bool,
) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let cwd = env::current_dir()?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or(cwd);
    let started = std::time::Instant::now();
    let args = serde_json::json!({
        "path": abs_path.to_string_lossy(),
        "line": line,
        "character": col,
        "new_name": new_name,
        "accessors": accessors,
        "comments": comments,
        "force": force,
    });
    let result = prod_code_mcp::tools::execute_tool(remote, &ws_root, "code_rename", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    println!("[{:.2}s]", started.elapsed().as_secs_f64());
    Ok(())
}

pub async fn run_codemod_cli(
    remote: SocketAddr,
    rule: String,
    path: Option<String>,
    apply: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({ "rule": rule, "apply": apply });
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    let result = prod_code_mcp::tools::execute_tool(remote, &root, "code_codemod", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

/// `check --fix` / `lint --fix`: apply the compiler's machine-applicable fixes, then run again.
pub async fn run_fix(
    remote: SocketAddr,
    kind: VerifyKind,
    timeout_secs: u64,
    json: bool,
    path: Option<PathBuf>,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let scope = verify_scope(&cwd, path.as_deref())?;
    let fixed =
        prod_code_mcp::fixit::check_and_fix(remote, &root, Some(&scope), kind, timeout_secs)
            .await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&fixed)?);
    } else {
        print!("{}", fixed.render(200));
    }
    std::process::exit(if fixed.ok() { 0 } else { 1 });
}
