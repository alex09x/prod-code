/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::DispatchContext;
use crate::cli::Commands;
use crate::commands::ops::*;
use crate::lsp_bridge::run_lsp_bridge;
use crate::update::run_update;
use crate::workspace::find_workspace_root;
use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;

pub async fn dispatch_ops(cmd: Commands, cx: &DispatchContext<'_>) -> Result<()> {
    match cmd {
        Commands::Lsp {
            reconnect,
            watchdog_secs,
            ..
        } => run_lsp_bridge(cx.remote, cx.lsp_engine, reconnect, watchdog_secs).await,
        Commands::Status { .. } => unreachable!("handled before placement"),
        Commands::Cluster { json, rebalance } => {
            run_cluster(cx.remotes, cx.placement_key, cx.cwd_engine, json, rebalance).await
        }
        Commands::Resolve { domain, json } => run_resolve(&domain, json).await,
        Commands::Metrics { since, json } => run_metrics(cx.remotes, since, json).await,
        Commands::ReportIssue { .. } => unreachable!("handled before placement"),
        Commands::Mcp => run_mcp_server(cx.remote).await,
        Commands::Sync { path, pull } => {
            if pull {
                let files = match path {
                    Some(p) => vec![p],
                    None => anyhow::bail!(
                        "--pull requires at least one file or path to pull (e.g. `prod-code sync --pull path/to/file.rs` or `prod-code pull <files...>`); to push current changes omit --pull"
                    ),
                };
                let root = cx
                    .cwd_root
                    .context("Failed to resolve workspace root")?;
                let current_dir = env::current_dir().unwrap_or_else(|_| root.to_path_buf());
                let files: Vec<PathBuf> = files
                    .into_iter()
                    .map(|f| if f.is_absolute() { f } else { current_dir.join(f) })
                    .collect();
                run_pull(cx.remote, root, files).await
            } else {
                run_sync(cx.remote, path).await
            }
        }
        Commands::Pull { files } => {
            let root = cx
                .cwd_root
                .context("Failed to resolve workspace root")?;
            let current_dir = env::current_dir().unwrap_or_else(|_| root.to_path_buf());
            let files: Vec<PathBuf> = files
                .into_iter()
                .map(|f| if f.is_absolute() { f } else { current_dir.join(f) })
                .collect();
            run_pull(cx.remote, root, files).await
        }
        Commands::ProposeExpression {
            file,
            line,
            target_type,
            json,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let abs = if file.is_absolute() {
                file
            } else {
                cwd.join(file)
            };
            let args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "line": line,
                "target_type": target_type,
            });
            if json {
                let report = prod_code_mcp::expression_synthesis::propose_expressions_in_scope(
                    &root,
                    &abs.to_string_lossy(),
                    line,
                    &target_type,
                )?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            } else {
                let result = prod_code_mcp::tools::execute_tool(
                    cx.remote,
                    &root,
                    "code_propose_expression",
                    args,
                )
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
        }
        Commands::Update { check, force, tag } => run_update(check, force, tag).await,
        Commands::Package { .. } | Commands::Cert { .. } => unreachable!(),
        _ => unreachable!(),
    }
}
