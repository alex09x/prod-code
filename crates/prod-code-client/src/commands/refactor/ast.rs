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
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub async fn run_encapsulate_field_cli(
    remote: SocketAddr,
    symbol: String,
    line: Option<u32>,
    character: u32,
    path: Option<String>,
    field: Option<String>,
    class: Option<String>,
    by_value: Option<bool>,
    verify: Option<String>,
    apply: bool,
    force: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({ "apply": apply, "force": force });
    let is_file_path =
        std::path::Path::new(&symbol).extension().is_some() || cwd.join(&symbol).exists();
    if let Some(line) = line {
        args["path"] = serde_json::Value::String(symbol);
        args["line"] = serde_json::Value::from(line);
        args["character"] = serde_json::Value::from(character);
    } else if is_file_path {
        args["path"] = serde_json::Value::String(symbol);
    } else if field.is_some() {
        if path.is_some() {
            args["class_name"] = serde_json::Value::String(symbol);
        } else {
            let matches =
                prod_code_mcp::tools::workspace_symbol_search(remote, &root, &symbol, None, 100)
                    .await?
                    .into_iter()
                    .filter(|hit| hit.name.eq_ignore_ascii_case(&symbol))
                    .map(|hit| hit.path)
                    .collect::<std::collections::BTreeSet<_>>();
            let file = match matches.len() {
                0 => anyhow::bail!(
                    "no class or struct named {symbol} was found; pass --path to the declaring file"
                ),
                1 => matches.into_iter().next().unwrap(),
                _ => anyhow::bail!(
                    "more than one class or struct named {symbol} was found; pass --path to select the declaring file"
                ),
            };
            args["path"] = serde_json::Value::String(file.to_string_lossy().into_owned());
            args["class_name"] = serde_json::Value::String(symbol);
        }
    } else {
        args["symbol"] = serde_json::Value::String(symbol);
    }
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    if let Some(field) = field {
        args["field"] = serde_json::Value::String(field);
    }
    if let Some(class) = class {
        args["class_name"] = serde_json::Value::String(class);
    }
    if let Some(by_value) = by_value {
        args["by_value"] = serde_json::Value::Bool(by_value);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_encapsulate_field", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn run_extract_parameter_cli(
    remote: SocketAddr,
    file: &Path,
    line: u32,
    character: u32,
    to: &str,
    name: String,
    ty: Option<String>,
    replace_all: bool,
    verify: Option<String>,
    apply: bool,
    force: bool,
) -> Result<()> {
    let (end_line, end_character): (u32, u32) = to
        .split_once(':')
        .and_then(|(l, c)| {
            let line = l.trim().parse::<u32>().ok()?;
            let col = c.trim().parse::<u32>().ok()?;
            Some((line, col))
        })
        .context("--to takes LINE:COL, for example --to 42:31")?;
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({
        "path": file.to_string_lossy(),
        "line": line,
        "character": character,
        "end_line": end_line,
        "end_character": end_character,
        "name": name,
        "replace_all": replace_all,
        "apply": apply,
        "force": force,
    });
    if let Some(ty) = ty {
        args["type"] = serde_json::Value::String(ty);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_extract_parameter", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn run_parameter_object_cli(
    remote: SocketAddr,
    symbol: String,
    params: Vec<String>,
    name: String,
    binding: Option<String>,
    path: Option<String>,
    verify: Option<String>,
    apply: bool,
    force: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({
        "symbol": symbol, "params": params, "name": name, "apply": apply, "force": force
    });
    if let Some(binding) = binding {
        args["binding"] = serde_json::Value::String(binding);
    }
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_introduce_parameter_object", args)
            .await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

pub async fn run_move_cli(
    remote: SocketAddr,
    symbol: String,
    to: String,
    path: Option<String>,
    verify: Option<String>,
    apply: bool,
    force: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args =
        serde_json::json!({ "symbol": symbol, "to": to, "apply": apply, "force": force });
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result = prod_code_mcp::tools::execute_tool(remote, &root, "code_move", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn run_change_signature_cli(
    remote: SocketAddr,
    symbol: String,
    params: Vec<String>,
    returns: Option<String>,
    visibility: Option<String>,
    asyncness: Option<bool>,
    path: Option<String>,
    verify: Option<String>,
    apply: bool,
    force: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args =
        serde_json::json!({ "symbol": symbol, "params": params, "apply": apply, "force": force });
    if let Some(returns) = returns {
        args["returns"] = serde_json::Value::String(returns);
    }
    if let Some(asyncness) = asyncness {
        args["async"] = serde_json::Value::Bool(asyncness);
    }
    if let Some(visibility) = visibility {
        args["visibility"] = serde_json::Value::String(visibility);
    }
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_change_signature", args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}
