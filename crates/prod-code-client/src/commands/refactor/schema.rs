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

#[allow(clippy::too_many_arguments)]
pub async fn run_schema_rename_cli(
    remote: SocketAddr,
    field: String,
    to: String,
    path: Option<String>,
    repos: Vec<String>,
    verify: Option<String>,
    apply: bool,
    force: bool,
    workspace_edit: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({
        "field": field,
        "to": to,
        "apply": apply,
        "force": force,
        "workspace_edit": workspace_edit,
    });
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    if !repos.is_empty() {
        let repos = repos
            .iter()
            .map(|r| cwd.join(r).to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        args["repos"] = serde_json::json!(repos);
    }
    if let Some(verify) = verify {
        args["verify"] = serde_json::Value::String(verify);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_schema_rename", args).await?;
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
pub async fn run_migrate_type_cli(
    remote: SocketAddr,
    symbol: String,
    to: String,
    line: Option<u32>,
    character: u32,
    path: Option<String>,
    convert: bool,
    transitive: bool,
    apply: bool,
    force: bool,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let mut args = serde_json::json!({ "to": to, "convert": convert, "transitive": transitive, "apply": apply, "force": force });
    match line {
        Some(line) => {
            args["path"] = serde_json::Value::String(symbol);
            args["line"] = serde_json::Value::from(line);
            args["character"] = serde_json::Value::from(character);
        }
        None => args["symbol"] = serde_json::Value::String(symbol),
    }
    if let Some(path) = path {
        args["path"] = serde_json::Value::String(path);
    }
    let result =
        prod_code_mcp::tools::execute_tool(remote, &root, "code_migrate_type", args).await?;
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
pub async fn run_fixture_cli(
    remote: SocketAddr,
    symbol: String,
    depth: u32,
    verify: bool,
    path: Option<String>,
    builder: bool,
    builder_name: Option<String>,
    randomized: bool,
    mock: bool,
    language: Option<String>,
) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let hint = path.map(|p| {
        let path = PathBuf::from(&p);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    });
    if builder {
        let preview = prod_code_mcp::fixture::builder::preview(
            remote,
            &root,
            &prod_code_mcp::fixture::builder::BuilderRequest {
                symbol: &symbol,
                hint: hint.as_deref(),
                builder_name: builder_name.as_deref(),
                verify,
            },
        )
        .await?;
        println!("{}", preview.render());
        anyhow::ensure!(
            !verify || preview.verified(),
            "builder verification did not succeed; nothing was written"
        );
        return Ok(());
    }
    let parsed_lang = language
        .as_deref()
        .and_then(|l| match l.to_ascii_lowercase().as_str() {
            "rust" | "rs" => Some(prod_code_mcp::parameter_object::Language::Rust),
            "go" | "golang" => Some(prod_code_mcp::parameter_object::Language::Go),
            "typescript" | "ts" => Some(prod_code_mcp::parameter_object::Language::TypeScript),
            "javascript" | "js" => Some(prod_code_mcp::parameter_object::Language::JavaScript),
            "python" | "py" => Some(prod_code_mcp::parameter_object::Language::Python),
            "c" => Some(prod_code_mcp::parameter_object::Language::C),
            "cpp" | "c++" => Some(prod_code_mcp::parameter_object::Language::Cpp),
            "swift" => Some(prod_code_mcp::parameter_object::Language::Swift),
            _ => None,
        });
    let fixture = prod_code_mcp::fixture::generate_with_options(
        remote,
        &root,
        &symbol,
        prod_code_mcp::fixture::FixtureOptions {
            depth,
            verify,
            hint,
            randomized,
            mock,
            language: parsed_lang,
        },
    )
    .await?;
    println!("{}", fixture.render());
    if !fixture.diagnostics.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}
