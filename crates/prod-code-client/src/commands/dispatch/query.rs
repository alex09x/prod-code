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
use crate::commands::common::{position, run_by_symbol, run_tool, symbol_args};
use crate::commands::query::*;
use crate::workspace::find_workspace_root;
use anyhow::{Context, Result};
use std::env;
use std::path::Path;

pub async fn dispatch_query(cmd: Commands, cx: &DispatchContext<'_>) -> Result<()> {
    match cmd {
        Commands::Def {
            file,
            line,
            col,
            symbol,
            body,
        } => match symbol {
            Some(symbol) => {
                let mut args = symbol_args(&symbol, file);
                args["body"] = serde_json::json!(body);
                run_tool(cx.remote, "code_definition", args).await
            }
            None if body => {
                let (file, line, col) = position(file, line, col)?;
                let file = std::fs::canonicalize(&file).unwrap_or(file);
                run_tool(
                    cx.remote,
                    "code_definition",
                    serde_json::json!({
                        "path": file.to_string_lossy(),
                        "line": line,
                        "character": col,
                        "body": true,
                    }),
                )
                .await
            }
            None => {
                let (file, line, col) = position(file, line, col)?;
                run_definition(cx.remote, &file, line, col).await
            }
        },
        Commands::Hover {
            file,
            line,
            col,
            symbol,
        } => match symbol {
            Some(symbol) => run_by_symbol(cx.remote, "code_hover", &symbol, file).await,
            None => {
                let (file, line, col) = position(file, line, col)?;
                run_hover(cx.remote, &file, line, col).await
            }
        },
        Commands::Refs {
            file,
            line,
            col,
            symbol,
            also_in,
        } => {
            let mut args = match symbol {
                Some(symbol) => symbol_args(&symbol, file),
                None => {
                    let (file, line, col) = position(file, line, col)?;
                    let file = std::fs::canonicalize(&file).unwrap_or(file);
                    serde_json::json!({
                        "path": file.to_string_lossy(),
                        "line": line,
                        "character": col,
                    })
                }
            };
            if !also_in.is_empty() {
                args["also_in"] = also_in
                    .into_iter()
                    .map(|dir| {
                        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
                        serde_json::json!(dir.to_string_lossy())
                    })
                    .collect();
            }
            run_refs(cx.remote, args).await
        }
        Commands::Callers {
            file,
            line,
            col,
            symbol,
            depth,
        } => run_call_tree(cx.remote, "code_callers", file, line, col, symbol, depth).await,
        Commands::Callees {
            file,
            line,
            col,
            symbol,
            depth,
        } => run_call_tree(cx.remote, "code_callees", file, line, col, symbol, depth).await,
        Commands::Impls {
            file,
            line,
            col,
            symbol,
        } => match symbol {
            Some(symbol) => run_by_symbol(cx.remote, "code_implementations", &symbol, file).await,
            None => {
                let (file, line, col) = position(file, line, col)?;
                run_implementations(cx.remote, &file, line, col).await
            }
        },
        Commands::Supertypes {
            file,
            line,
            col,
            symbol,
            depth,
        } => match symbol {
            Some(symbol) => {
                let mut args = symbol_args(&symbol, file);
                args["depth"] = serde_json::json!(depth);
                run_tool(cx.remote, "code_supertypes", args).await
            }
            None => {
                let (file, line, col) = position(file, line, col)?;
                let file = std::fs::canonicalize(&file).unwrap_or(file);
                run_tool(
                    cx.remote,
                    "code_supertypes",
                    serde_json::json!({
                        "path": file.to_string_lossy(),
                        "line": line,
                        "character": col,
                        "depth": depth,
                    }),
                )
                .await
            }
        },
        Commands::Symbols { target } => {
            if Path::new(&target).is_file() {
                run_symbols(
                    cx.remote,
                    Path::new(&target),
                    &prod_code_mcp::tools::OutlineOptions::all(usize::MAX, false, "pass --locals"),
                )
                .await
            } else {
                run_tool(
                    cx.remote,
                    "code_symbols",
                    serde_json::json!({ "query": target }),
                )
                .await
            }
        }
        Commands::Outline {
            file,
            locals,
            kinds,
            exported,
            max_bytes,
            max_items,
        } => {
            let is_dir = file.is_dir();
            let options = prod_code_mcp::tools::OutlineOptions {
                max_depth: usize::MAX,
                include_locals: locals,
                hint: "pass --locals".to_string(),
                kinds: (!kinds.is_empty()).then_some(kinds),
                exported_only: exported,
                max_bytes: match max_bytes {
                    Some(0) => None,
                    Some(bytes) => Some(bytes),
                    None => is_dir.then_some(prod_code_mcp::tools::DIRECTORY_OUTLINE_BYTES),
                },
                max_items: max_items.filter(|n| *n > 0),
            };
            run_symbols(cx.remote, &file, &options).await
        }
        Commands::Source {
            path,
            line,
            context,
        } => run_source(cx.remote, cx.cwd_root, &path, line, context).await,
        Commands::Impact {
            base,
            depth,
            run,
            ci,
            json,
        } => run_impact(cx.remote, base.as_deref(), depth, run, ci, json).await,
        Commands::DeadCode {
            include_exported,
            reachability,
            max_files,
            json,
        } => run_dead_code(cx.remote, include_exported, reachability, max_files, json).await,
        Commands::Prune {
            max_files,
            apply,
            force,
            reachability,
            patch,
            commit,
            json,
        } => {
            run_prune(
                cx.remote,
                max_files,
                reachability,
                apply,
                force,
                patch,
                commit,
                json,
            )
            .await
        }
        Commands::Diagnostics { file, json } => run_diagnostics(cx.remote, &file, None, json).await,
        Commands::Diagnose {
            filter,
            timeout_secs,
            json,
        } => run_diagnose(cx.remote, filter.as_deref(), timeout_secs, json).await,
        Commands::Search { query, limit, path } => run_search_cli(cx.remote, query, limit, path).await,
        Commands::Slice {
            target,
            line,
            character,
            depth,
            max_bytes,
            dataflow,
            target_line,
            target_var,
        } => {
            let options = prod_code_mcp::slice::SliceOptions {
                depth,
                max_bytes,
                dataflow,
                target_line,
                target_var,
            };
            run_slice(cx.remote, target, line, character, options).await
        }
        Commands::Dependencies { scope, path, json } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "scope": scope,
            });
            if let Some(p) = path.as_ref() {
                args["path"] = serde_json::Value::String(p.to_string_lossy().into_owned());
            }
            if json {
                let dep_scope = match scope.as_str() {
                    "modules" => prod_code_mcp::dependencies::DependencyScope::Modules,
                    _ => prod_code_mcp::dependencies::DependencyScope::Crates,
                };
                let report = prod_code_mcp::dependencies::analyze_dependencies(
                    &root,
                    dep_scope,
                    path.as_deref(),
                )?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            } else {
                let result =
                    prod_code_mcp::tools::execute_tool(cx.remote, &root, "code_dependencies", args)
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
        Commands::Duplicates {
            min_lines,
            parameterized,
            type3,
            max_groups,
            path,
            json,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "min_lines": min_lines,
                "parameterized": parameterized,
                "type3": type3,
                "max_groups": max_groups,
            });
            if let Some(p) = path.as_ref() {
                args["path"] = serde_json::Value::String(p.to_string_lossy().into_owned());
            }
            if json {
                let options = prod_code_mcp::duplicates::DuplicateOptions {
                    min_lines,
                    parameterized,
                    type3,
                    max_groups,
                };
                let report =
                    prod_code_mcp::duplicates::find_duplicates(&root, path.as_deref(), options)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            } else {
                let result =
                    prod_code_mcp::tools::execute_tool(cx.remote, &root, "code_find_duplicates", args)
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
        Commands::StructuralSearch {
            pattern,
            path,
            json,
        } => {
            let cwd = env::current_dir().context("Failed to get current working directory")?;
            let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
            let mut args = serde_json::json!({
                "pattern": pattern,
            });
            if let Some(p) = path.as_ref() {
                args["path"] = serde_json::Value::String(p.to_string_lossy().into_owned());
            }
            if json {
                let report = prod_code_mcp::codemod::run_structural_search(
                    &root,
                    &pattern,
                    path.as_deref(),
                )?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                Ok(())
            } else {
                let result = prod_code_mcp::tools::execute_tool(
                    cx.remote,
                    &root,
                    "code_structural_search",
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
        _ => unreachable!(),
    }
}
