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
use crate::commands::common::run_tool;
use crate::workspace::find_workspace_root;
use anyhow::{Context, Result};
use std::env;
use std::net::SocketAddr;

async fn exec_tool(remote: SocketAddr, tool_name: &str, args: serde_json::Value) -> Result<()> {
    let cwd = env::current_dir().context("Failed to get current working directory")?;
    let root = find_workspace_root(&cwd).unwrap_or_else(|| cwd.clone());
    let result = prod_code_mcp::tools::execute_tool(remote, &root, tool_name, args).await?;
    for content in &result.content {
        let prod_code_mcp::protocol::McpContentItem::Text { text } = content;
        println!("{text}");
    }
    if result.is_error {
        std::process::exit(1);
    }
    Ok(())
}

pub async fn dispatch_refactor_members(cmd: Commands, cx: &DispatchContext<'_>) -> Result<()> {
    match cmd {
        Commands::ExtractFunction {
            file,
            line,
            col,
            to,
            name,
            no_duplicates,
            parameterize,
            other_files,
            verify,
            apply,
            force,
        } => {
            let (end_line, end_col) = to
                .split_once(':')
                .and_then(|(l, c)| Some((l.parse::<u32>().ok()?, c.parse::<u32>().ok()?)))
                .context("--to takes LINE:COL")?;
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            let mut args = serde_json::json!({
                "path": abs.to_string_lossy(),
                "line": line,
                "character": col,
                "end_line": end_line,
                "end_character": end_col,
                "name": name,
                "duplicates": !no_duplicates,
                "parameterize": parameterize,
                "other_files": other_files,
                "apply": apply,
                "force": force,
            });
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            run_tool(cx.remote, "code_extract_function", args).await
        }
        Commands::IntroduceVariable {
            file,
            line,
            col,
            to,
            name,
            apply,
            force,
        } => {
            let (end_line, end_col) = to
                .split_once(':')
                .and_then(|(l, c)| Some((l.parse::<u32>().ok()?, c.parse::<u32>().ok()?)))
                .context("--to takes LINE:COL")?;
            let abs = std::fs::canonicalize(&file).unwrap_or(file);
            run_tool(
                cx.remote,
                "code_introduce_variable",
                serde_json::json!({
                    "path": abs.to_string_lossy(),
                    "line": line,
                    "character": col,
                    "end_line": end_line,
                    "end_character": end_col,
                    "name": name,
                    "apply": apply,
                    "force": force,
                }),
            )
            .await
        }
        Commands::ReplaceConstructorWithFactory {
            file,
            type_name,
            name,
            verify,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "type_name": type_name,
                "apply": apply,
                "force": force,
            });
            if let Some(n) = name {
                args["factory_name"] = serde_json::Value::String(n);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            exec_tool(cx.remote, "code_replace_constructor_with_factory", args).await
        }
        Commands::ReplaceConstructorWithBuilder {
            file,
            type_name,
            name,
            verify,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "type_name": type_name,
                "apply": apply,
                "force": force,
            });
            if let Some(n) = name {
                args["builder_name"] = serde_json::Value::String(n);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            exec_tool(cx.remote, "code_replace_constructor_with_builder", args).await
        }
        Commands::PullUp {
            file,
            class,
            members,
            target_class,
            no_clean_siblings,
            verify,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "class_name": class,
                "members": members,
                "clean_siblings": !no_clean_siblings,
                "apply": apply,
                "force": force,
            });
            if let Some(t) = target_class {
                args["target_class"] = serde_json::Value::String(t);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            exec_tool(cx.remote, "code_pull_up", args).await
        }
        Commands::PushDown {
            file,
            class,
            members,
            target_classes,
            verify,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "class_name": class,
                "members": members,
                "apply": apply,
                "force": force,
            });
            if !target_classes.is_empty() {
                args["target_classes"] = serde_json::Value::Array(
                    target_classes
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                );
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            exec_tool(cx.remote, "code_push_down", args).await
        }
        Commands::ReplaceInheritanceWithDelegation {
            file,
            sub_type,
            base_type,
            field_name,
            methods,
            verify,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "sub_type": sub_type,
                "apply": apply,
                "force": force,
            });
            if let Some(b) = base_type {
                args["base_type"] = serde_json::Value::String(b);
            }
            if let Some(f) = field_name {
                args["field_name"] = serde_json::Value::String(f);
            }
            if !methods.is_empty() {
                args["methods"] = serde_json::Value::Array(
                    methods.into_iter().map(serde_json::Value::String).collect(),
                );
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            exec_tool(cx.remote, "code_replace_inheritance_with_delegation", args).await
        }
        Commands::ReplaceConditionalWithPolymorphism {
            file,
            line,
            character,
            base_name,
            method_name,
            params,
            return_type,
            target_var,
            verify,
            apply,
            force,
        } => {
            anyhow::ensure!(
                line > 0 && character > 0,
                "line and character must be one-based coordinates"
            );
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "base_name": base_name,
                "method_name": method_name,
                "line": line,
                "character": character,
                "apply": apply,
                "force": force,
            });
            if !params.is_empty() {
                args["params"] = serde_json::Value::Array(
                    params.into_iter().map(serde_json::Value::String).collect(),
                );
            }
            if let Some(r) = return_type {
                args["return_type"] = serde_json::Value::String(r);
            }
            if let Some(t) = target_var {
                args["target_var"] = serde_json::Value::String(t);
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            exec_tool(cx.remote, "code_replace_conditional_with_polymorphism", args).await
        }
        Commands::ExtractInterface {
            file,
            symbol,
            name,
            methods,
            line,
            character,
            no_migrate_callers,
            verify,
            apply,
            force,
        } => {
            if file.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                anyhow::ensure!(
                    line.is_some_and(|value| value > 0) && character.is_some_and(|value| value > 0),
                    "Rust extract-interface requires one-based --line and --character positions"
                );
            }
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "symbol": symbol,
                "interface_name": name,
                "migrate_callers": !no_migrate_callers,
                "apply": apply,
                "force": force,
            });
            if let Some(line) = line {
                anyhow::ensure!(line > 0, "line must be a one-based coordinate");
                args["line"] = serde_json::json!(line);
            }
            if let Some(character) = character {
                anyhow::ensure!(character > 0, "character must be a one-based coordinate");
                args["character"] = serde_json::json!(character);
            }
            if !methods.is_empty() {
                args["methods"] = serde_json::Value::Array(
                    methods.into_iter().map(serde_json::Value::String).collect(),
                );
            }
            if let Some(v) = verify {
                args["verify"] = serde_json::Value::String(v);
            }
            exec_tool(cx.remote, "code_extract_interface", args).await
        }
        Commands::ExtractField {
            file,
            line,
            character,
            to,
            range,
            expression,
            name,
            ty,
            init,
            replace_all,
            verify,
            apply,
            force,
        } => {
            let mut args = serde_json::json!({
                "path": file.to_string_lossy(),
                "name": name,
                "replace_all": replace_all,
                "apply": apply,
                "force": force,
            });
            if let Some(l) = line {
                args["line"] = serde_json::Value::Number(l.into());
            }
            if let Some(c) = character {
                args["character"] = serde_json::Value::Number(c.into());
            }
            if let Some(to_pos) = to {
                let (end_line, end_character): (u32, u32) = to_pos
                    .split_once(':')
                    .and_then(|(l, c)| Some((l.trim().parse().ok()?, c.trim().parse().ok()?)))
                    .context("--to takes LINE:COL, for example --to 42:31")?;
                args["end_line"] = serde_json::Value::Number(end_line.into());
                args["end_character"] = serde_json::Value::Number(end_character.into());
            }
            if let Some(r) = range {
                args["range"] = serde_json::Value::String(r);
            }
            if let Some(expr) = expression {
                args["expression"] = serde_json::Value::String(expr);
            }
            if let Some(t) = ty {
                args["type"] = serde_json::Value::String(t);
            }
            if let Some(init) = init {
                args["init"] = serde_json::Value::String(init);
            }
            if let Some(verify) = verify {
                args["verify"] = serde_json::Value::String(verify);
            }
            exec_tool(cx.remote, "code_extract_field", args).await
        }
        _ => unreachable!(),
    }
}
