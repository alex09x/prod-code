/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::protocol::{McpTool, McpToolCallResult};
use crate::report::ReportRequest;
use crate::sync::{push_workspace_sync, workspace_identity};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use std::net::SocketAddr;
use std::path::Path;
use tokio_util::codec::Framed;
use url::Url;

/// Return list of tools exposed by the MCP server.
pub fn list_tools() -> Vec<McpTool> {
    static CACHED: std::sync::OnceLock<Vec<McpTool>> = std::sync::OnceLock::new();
    CACHED
        .get_or_init(|| {
            std::thread::Builder::new()
                .name("build-tools".into())
                .stack_size(8 * 1024 * 1024)
                .spawn(build_tools_raw)
                .expect("spawn build_tools thread")
                .join()
                .expect("build_tools succeeded")
        })
        .clone()
}

pub(crate) mod schemas;
pub use schemas::{
    COMPILE_DESCRIPTION, POSITION_ARGUMENTS, SYMBOL_ADDRESSABLE, build_tools_raw,
    relax_position_schema,
};

pub mod outline;
pub use outline::{
    DIRECTORY_OUTLINE_BYTES, OutlineOptions, handle_outline, outline_directory, outline_file,
    protobuf_outline, render_outline,
};

pub(crate) fn checked_position_argument(value: &serde_json::Value, name: &str) -> Result<u32> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .with_context(|| {
            format!(
                "'{name}' must be a one-based coordinate in 1..={}",
                u32::MAX
            )
        })
}

/// Check only position properties the tool actually advertises. Keeping this registry derived
/// from discovery means a newly exposed position tool gets the same input contract immediately.
fn validate_position_arguments(tool_name: &str, args: &serde_json::Value) -> Result<()> {
    static POSITION_TOOLS: std::sync::OnceLock<Vec<(String, Vec<&'static str>)>> =
        std::sync::OnceLock::new();
    let positions = POSITION_TOOLS.get_or_init(|| {
        list_tools()
            .into_iter()
            .filter_map(|tool| {
                let properties = tool.input_schema.get("properties")?.as_object()?;
                let fields: Vec<_> = POSITION_ARGUMENTS
                    .into_iter()
                    .filter(|name| properties.contains_key(*name))
                    .collect();
                (!fields.is_empty()).then_some((tool.name, fields))
            })
            .collect()
    });
    let Some((_, fields)) = positions.iter().find(|(name, _)| name == tool_name) else {
        return Ok(());
    };
    for name in fields {
        if let Some(value) = args.get(*name) {
            checked_position_argument(value, name)?;
        }
    }
    if fields.contains(&"end_line") && fields.contains(&"end_character") {
        let end = match (args.get("end_line"), args.get("end_character")) {
            (None, None) => return Ok(()),
            (Some(line), Some(character)) => (
                checked_position_argument(line, "end_line")?,
                checked_position_argument(character, "end_character")?,
            ),
            _ => anyhow::bail!("a selection requires both 'end_line' and 'end_character'"),
        };
        if let (Some(line), Some(character)) = (args.get("line"), args.get("character")) {
            let start = (
                checked_position_argument(line, "line")?,
                checked_position_argument(character, "character")?,
            );
            anyhow::ensure!(end >= start, "selection end precedes its start");
        }
    }
    Ok(())
}

/// Execute an MCP tool call against the remote gateway.
pub async fn execute_tool(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: serde_json::Value,
) -> Result<McpToolCallResult> {
    Box::pin(execute_tool_inner(remote, workspace_root, tool_name, args)).await
}

async fn execute_tool_inner(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: serde_json::Value,
) -> Result<McpToolCallResult> {
    // Invalid explicit positions are refused before symbol lookup, routing or any write.
    validate_position_arguments(tool_name, &args)?;
    // Notes left by an earlier call are not this answer's (#391).
    let _ = crate::session::take_indexing_notes(workspace_root);
    if tool_name == "code_references"
        && let Some(dirs) = args.get("also_in").and_then(|v| v.as_array())
        && !dirs.is_empty()
    {
        let dirs = dirs.clone();
        return references_across(remote, workspace_root, args, &dirs).await;
    }
    // Normalize path parameter across aliases: path, file_path, file, package, crate (#673, #900)
    let mut args = args;
    let initial_path = args
        .get("path")
        .or_else(|| args.get("file_path"))
        .or_else(|| args.get("file"))
        .or_else(|| args.get("package"))
        .or_else(|| args.get("crate"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if let Some(ref p) = initial_path
        && let Some(obj) = args.as_object_mut()
        && !obj.contains_key("path")
    {
        obj.insert("path".into(), serde_json::Value::String(p.clone()));
    }

    // A path in a nested project of another language goes to a node that serves it (#125, #673).
    // Route before resolving symbols so cross-language symbol lookups hit the right language engine.
    let remote =
        crate::cluster::route_for_path(remote, workspace_root, initial_path.as_deref()).await?;

    // `symbol` instead of line/character: resolve the name through the workspace symbol
    // index, then run the tool at that position.
    let args = if SYMBOL_ADDRESSABLE.contains(&tool_name)
        && let Some(symbol) = args.get("symbol").and_then(|v| v.as_str())
        && !symbol.trim().is_empty()
    {
        let hint = args
            .get("path")
            .and_then(|v| v.as_str())
            .filter(|p| !p.trim().is_empty())
            .map(|p| resolve_file_path(workspace_root, p));
        let hit = resolve_symbol(remote, workspace_root, symbol.trim(), hint.as_deref()).await?;
        let mut owned = args.clone();
        if let Some(obj) = owned.as_object_mut() {
            obj.insert(
                "path".into(),
                serde_json::Value::String(hit.path.to_string_lossy().into_owned()),
            );
            obj.insert("line".into(), serde_json::json!(hit.line));
            obj.insert("character".into(), serde_json::json!(hit.col));
        }
        owned
    } else {
        args
    };
    // Resolved symbol positions obey the same contract, including a supplied selection end.
    validate_position_arguments(tool_name, &args)?;
    // A path in a nested project of another language goes to a node that serves it (#125).
    let remote = crate::cluster::route_for_path(
        remote,
        workspace_root,
        args.get("path").and_then(|v| v.as_str()),
    )
    .await?;
    let result = Box::pin(dispatch_tool(remote, workspace_root, tool_name, args)).await;
    with_indexing_notes(result, workspace_root)
}

async fn dispatch_tool(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: serde_json::Value,
) -> Result<McpToolCallResult> {
    match tool_name {
        "code_symbols" => handle_symbols(remote, workspace_root, &args).await,
        "code_safe_delete" => handle_safe_delete(remote, workspace_root, &args).await,
        "code_assists" | "code_assist" => {
            handle_assists(remote, workspace_root, tool_name, &args).await
        }
        "code_check" | "code_lint" | "code_test" | "code_benchmarks" => {
            handle_check(remote, workspace_root, tool_name, &args).await
        }
        "code_rename" => handle_rename(remote, workspace_root, &args).await,
        "code_exec" => handle_exec(remote, workspace_root, &args).await,
        "code_definition" => handle_definition(remote, workspace_root, &args).await,

        "code_callers" | "code_callees" => {
            handle_callers(remote, workspace_root, tool_name, &args).await
        }
        "code_implementations" => handle_implementations(remote, workspace_root, &args).await,
        "code_supertypes" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let num = |key: &str| -> Result<u32> {
                args.get(key)
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .with_context(|| format!("Missing '{key}' argument"))
            };
            let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            let file_path = resolve_file_path(workspace_root, path_str);
            let found = crate::supertypes::supertypes(
                remote,
                workspace_root,
                &file_path,
                num("line")?,
                num("character")?,
                depth,
            )
            .await?;
            Ok(McpToolCallResult::text(found.render(workspace_root)))
        }
        "code_impact" => {
            let base = args.get("base").and_then(|v| v.as_str());
            let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(4) as usize;
            let report = crate::impact::analyze(remote, workspace_root, base, depth).await?;
            Ok(McpToolCallResult::text(report.render()))
        }
        "code_diagnose_failure" => handle_diagnose_failure(remote, workspace_root, &args).await,
        "code_diagnostics" | "code_validate_edit" => {
            handle_diagnostics(remote, workspace_root, tool_name, &args).await
        }
        "code_validate_edits" => handle_validate_edits(remote, workspace_root, &args).await,
        "code_shadow_run" => handle_shadow_run(remote, workspace_root, &args).await,
        "code_slice" => handle_slice(remote, workspace_root, &args).await,
        "code_search" => handle_search(remote, workspace_root, &args).await,
        "code_codemod" => handle_codemod(remote, workspace_root, &args).await,
        "code_schema_rename" => handle_schema_rename(remote, workspace_root, &args).await,
        "code_encapsulate_field" => handle_encapsulate_field(remote, workspace_root, &args).await,
        "code_migrate_type" => handle_migrate_type(remote, workspace_root, &args).await,
        "code_extract_field" => handle_extract_field(remote, workspace_root, &args).await,
        "code_wrap_return" => handle_wrap_return(remote, workspace_root, &args).await,
        "code_make_static" => handle_make_static(remote, workspace_root, &args).await,
        "code_inline_parameter" => handle_inline_parameter(remote, workspace_root, &args).await,
        "code_introduce_variable" => handle_introduce_variable(remote, workspace_root, &args).await,
        "code_extract_function" => handle_extract_function(remote, workspace_root, &args).await,
        "code_loop_to_iterator" => {
            let path_str = args
                .get("path")
                .or_else(|| args.get("file"))
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let symbol = args
                .get("symbol")
                .or_else(|| args.get("function"))
                .and_then(|v| v.as_str());
            let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
            let character = args
                .get("character")
                .or_else(|| args.get("col"))
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let file_path = resolve_file_path(workspace_root, path_str);
            let done = crate::loop_to_iterator::loop_to_iterator_polyglot(
                remote,
                workspace_root,
                &file_path,
                symbol,
                line,
                character,
                apply,
                force,
            )
            .await?;
            let text = done.render();
            Ok(if done.diagnostics.is_empty() {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            })
        }
        "code_extract_trait" => handle_extract_trait(remote, workspace_root, &args).await,
        "code_replace_constructor_with_factory" => {
            handle_replace_constructor_with_factory(remote, workspace_root, &args).await
        }
        "code_replace_constructor_with_builder" => {
            handle_replace_constructor_with_builder(remote, workspace_root, &args).await
        }
        "code_replace_constructor" => {
            let mode = args
                .get("mode")
                .and_then(|m| m.as_str())
                .unwrap_or("factory");
            if mode == "builder" {
                handle_replace_constructor_with_builder(remote, workspace_root, &args).await
            } else {
                handle_replace_constructor_with_factory(remote, workspace_root, &args).await
            }
        }
        "code_pull_up" => handle_pull_up(remote, workspace_root, &args).await,
        "code_push_down" => handle_push_down(remote, workspace_root, &args).await,
        "code_replace_inheritance_with_delegation" | "code_replace_inheritance" => {
            handle_replace_inheritance_with_delegation(remote, workspace_root, &args).await
        }
        "code_replace_conditional_with_polymorphism" | "code_replace_conditional" => {
            handle_replace_conditional_with_polymorphism(remote, workspace_root, &args).await
        }
        "code_extract_interface" => handle_extract_interface(remote, workspace_root, &args).await,
        "code_extract_delegate" => handle_extract_delegate(remote, workspace_root, &args).await,
        "code_convert_to_method" => handle_convert_to_method(remote, workspace_root, &args).await,
        "code_invert_boolean" => handle_invert_boolean(remote, workspace_root, &args).await,
        "code_generify" => handle_generify(remote, workspace_root, &args).await,
        "code_extract_parameter" => handle_extract_parameter(remote, workspace_root, &args).await,
        "code_introduce_parameter_object" => {
            handle_introduce_parameter_object(remote, workspace_root, &args).await
        }
        "code_move" => handle_move(remote, workspace_root, &args).await,
        "code_move_module" => handle_move_module(remote, workspace_root, &args).await,
        "code_move_method" => handle_move_method(remote, workspace_root, &args).await,
        "code_change_signature" => handle_change_signature(remote, workspace_root, &args).await,
        "code_generate_fixture" => handle_generate_fixture(remote, workspace_root, &args).await,
        "code_dead_code" => handle_dead_code(remote, workspace_root, &args).await,
        "code_prune_orphans" => {
            let max_files = args
                .get("max_files")
                .and_then(|v| v.as_u64())
                .unwrap_or(400) as usize;
            let mut apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let reachability = args
                .get("reachability")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let git_patch = args
                .get("git_patch")
                .or_else(|| args.get("patch"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let commit = args
                .get("commit")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if commit {
                apply = true;
            }
            let pruned = crate::prune::prune_orphans_opts(
                remote,
                workspace_root,
                crate::dead_code::DeadCodeOptions {
                    include_exported: false,
                    max_files,
                    reachability,
                },
                apply,
                force,
                git_patch,
                commit,
            )
            .await?;
            let text = if git_patch && !apply && !commit {
                pruned.git_patch.clone().unwrap_or_else(|| pruned.render())
            } else {
                pruned.render()
            };
            Ok(if pruned.diagnostics.is_empty() {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            })
        }
        "code_source" => handle_source(remote, workspace_root, &args).await,
        "code_references" => handle_references(remote, workspace_root, &args).await,

        "code_outline" => handle_outline(remote, workspace_root, &args).await,

        "code_hover" | "code_type_at" => handle_hover(remote, workspace_root, &args).await,

        "code_status" => handle_status(remote).await,
        "code_report_issue" => {
            let text = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("");
            let flag = |key: &str| args.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
            let labels: Vec<String> = args
                .get("labels")
                .and_then(|v| v.as_array())
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(|l| l.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            match crate::report::report(
                Some(remote),
                ReportRequest {
                    title: text("title"),
                    body: text("body"),
                    force: flag("force"),
                    dry_run: flag("dry_run"),
                    private_ref: args.get("private_ref").and_then(|v| v.as_str()),
                    labels: &labels,
                },
                &crate::report::gh_program(),
            )
            .await
            {
                Ok(outcome) => Ok(McpToolCallResult::text(outcome.render())),
                Err(err) => Ok(McpToolCallResult::error(format!("{err:#}"))),
            }
        }

        "code_sync" => handle_sync(remote, workspace_root, args).await,

        "code_dependencies" => handle_dependencies(workspace_root, &args).await,
        "code_find_duplicates" => handle_find_duplicates(workspace_root, &args).await,
        "code_structural_search" => handle_structural_search(workspace_root, &args).await,
        "code_propose_expression" => handle_propose_expression(workspace_root, &args).await,

        unknown => Ok(McpToolCallResult::error(format!("Unknown tool: {unknown}"))),
    }
}

async fn handle_dependencies(
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

async fn handle_find_duplicates(
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

async fn handle_structural_search(
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

async fn handle_propose_expression(
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

async fn handle_sync(
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

async fn handle_status(remote: SocketAddr) -> Result<McpToolCallResult> {
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

async fn handle_hover(
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

pub(crate) mod references;
pub(crate) use references::*;

async fn handle_source(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args.get("line").and_then(|v| v.as_u64()).map(|l| l as u32);
    let context = args.get("context").and_then(|v| v.as_u64()).unwrap_or(30) as u32;
    let (bytes, truncated) = crate::remote_fs::read_source(remote, workspace_root, path).await?;
    let text = String::from_utf8_lossy(&bytes);
    let mut out = match line {
        Some(line) => crate::remote_fs::snippet(&text, line, context),
        None => text.into_owned(),
    };
    if truncated {
        out.push_str("\n[truncated at 2 MiB]");
    }
    Ok(McpToolCallResult::text(out))
}

async fn handle_dead_code(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let include_exported = args
        .get("include_exported")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let reachability = args
        .get("reachability")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_files = args
        .get("max_files")
        .and_then(|v| v.as_u64())
        .unwrap_or(400) as usize;
    let report = crate::dead_code::find_dead_code_opts(
        remote,
        workspace_root,
        crate::dead_code::DeadCodeOptions {
            include_exported,
            max_files,
            reachability,
        },
    )
    .await?;
    Ok(McpToolCallResult::text(report.render()))
}

async fn handle_generate_fixture(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let symbol = args
        .get("symbol")
        .and_then(|v| v.as_str())
        .context("Missing 'symbol' argument")?;
    let builder = args
        .get("builder")
        .map(|value| value.as_bool().context("'builder' must be a boolean"))
        .transpose()?
        .unwrap_or(false);
    let builder_name = args
        .get("builder_name")
        .map(|value| value.as_str().context("'builder_name' must be a string"))
        .transpose()?;
    anyhow::ensure!(
        builder || builder_name.is_none(),
        "'builder_name' requires builder=true"
    );
    anyhow::ensure!(
        !builder || args.get("depth").is_none(),
        "'depth' applies to value fixtures only; omit it for builder=true"
    );
    let verify = args.get("verify").and_then(|v| v.as_bool()).unwrap_or(true);
    if builder {
        anyhow::ensure!(
            args.get("verify").is_none_or(|v| v.is_boolean()),
            "'verify' must be a boolean"
        );
    }
    let hint = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    if builder {
        let preview = crate::fixture::builder::preview(
            remote,
            workspace_root,
            &crate::fixture::builder::BuilderRequest {
                symbol,
                hint: hint.as_deref(),
                builder_name,
                verify,
            },
        )
        .await?;
        return Ok(if !verify || preview.verified() {
            McpToolCallResult::text(preview.render())
        } else {
            McpToolCallResult::error(preview.render())
        });
    }
    let randomized = args
        .get("randomized")
        .map(|value| value.as_bool().context("'randomized' must be a boolean"))
        .transpose()?
        .unwrap_or(false);
    let mock = args
        .get("mock")
        .map(|value| value.as_bool().context("'mock' must be a boolean"))
        .transpose()?
        .unwrap_or(false);
    anyhow::ensure!(!builder || !mock, "'mock' conflicts with builder=true");
    let language = args.get("language").and_then(|v| v.as_str()).and_then(|s| {
        match s.to_ascii_lowercase().as_str() {
            "rust" | "rs" => Some(crate::parameter_object::Language::Rust),
            "go" | "golang" => Some(crate::parameter_object::Language::Go),
            "typescript" | "ts" => Some(crate::parameter_object::Language::TypeScript),
            "javascript" | "js" => Some(crate::parameter_object::Language::JavaScript),
            "python" | "py" => Some(crate::parameter_object::Language::Python),
            "c" => Some(crate::parameter_object::Language::C),
            "cpp" | "c++" => Some(crate::parameter_object::Language::Cpp),
            "swift" => Some(crate::parameter_object::Language::Swift),
            _ => None,
        }
    });

    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(crate::fixture::DEFAULT_DEPTH as u64) as u32;
    let fixture = crate::fixture::generate_with_options(
        remote,
        workspace_root,
        symbol,
        crate::fixture::FixtureOptions {
            depth,
            verify,
            hint,
            randomized,
            mock,
            language,
        },
    )
    .await?;
    let clean = fixture.diagnostics.is_empty();
    let text = fixture.render();
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) mod refactor_handlers;
pub(crate) use refactor_handlers::*;

async fn handle_search(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .context("Missing 'query' argument")?;
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let subpath = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p))
        .and_then(|p| crate::exec::subdir_of(workspace_root, &p));
    let resp =
        crate::search::search(remote, workspace_root, query, limit, subpath.as_deref()).await?;
    Ok(McpToolCallResult::text(crate::search::render(&resp, query)))
}

async fn handle_slice(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let (file_path, line, character, symbol_based) =
        if let Some(sym) = args.get("symbol").and_then(|v| v.as_str()) {
            let hint = args.get("path").and_then(|v| v.as_str()).map(Path::new);
            let hit = resolve_symbol(remote, workspace_root, sym, hint).await?;
            (hit.path, hit.line, hit.col, true)
        } else {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument (or pass 'symbol')")?;
            let line =
                args.get("line")
                    .and_then(|v| v.as_u64())
                    .context("Missing 'line' argument (or pass 'symbol')")? as u32;
            let character = args.get("character").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
            (
                resolve_file_path(workspace_root, path_str),
                line,
                character,
                false,
            )
        };

    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(crate::slice::DEFAULT_DEPTH as u64) as u32;
    let max_bytes = args
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(crate::slice::DEFAULT_MAX_BYTES as u64) as usize;
    let dataflow = args
        .get("dataflow")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let target_line = args
        .get("target_line")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
        .or_else(|| (dataflow && !symbol_based).then_some(line));
    let target_var = args
        .get("target_var")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let options = crate::slice::SliceOptions {
        depth,
        max_bytes,
        dataflow,
        target_line,
        target_var,
    };
    let report = crate::slice::slice_with_options(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        options,
    )
    .await?;
    let rendered = report.render();
    if report.items.is_empty() {
        return Ok(McpToolCallResult::error(format!(
            "no slice items found: {rendered}"
        )));
    }
    Ok(McpToolCallResult::text(rendered))
}

async fn handle_shadow_run(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let argv: Vec<String> = args
        .get("argv")
        .and_then(|v| v.as_array())
        .context("Missing 'argv' argument")?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    if argv.is_empty() {
        return Ok(McpToolCallResult::error("'argv' is empty".to_string()));
    }
    let specs = crate::shadow::parse_specs(workspace_root, args, None)?;
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let parallel = args.get("parallel").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let tail_bytes = args
        .get("tail_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(16 * 1024) as usize;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let in_memory = args
        .get("in_memory")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || args.get("ram").and_then(|v| v.as_bool()).unwrap_or(false);
    let subdir = resolve_exec_subdir(workspace_root, args.get("cwd").and_then(|v| v.as_str()))?;
    let outcome = crate::shadow::run_shadow(
        remote,
        workspace_root,
        subdir.as_deref(),
        &specs,
        argv.clone(),
        vec![("CARGO_TERM_COLOR".to_string(), "never".to_string())],
        timeout_secs,
        parallel,
        tail_bytes,
        in_memory,
    )
    .await?;
    let applied = match (apply, outcome.winner) {
        (true, Some(i)) => Some(crate::shadow::apply_hypothesis(workspace_root, &specs[i])?),
        _ => None,
    };
    let text = crate::shadow::render_report(&outcome, &argv, applied.as_deref(), tail_bytes);
    Ok(if outcome.winner.is_some() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_validate_edits(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    // A patch or a WorkspaceEdit becomes whole files first: the overlay takes files.
    let mut notes = String::new();
    let edits: Vec<(std::path::PathBuf, String)> =
        if let Some(diff) = args.get("diff").and_then(|v| v.as_str()) {
            let patched = crate::patch::apply(workspace_root, diff)?;
            for gone in &patched.deleted {
                notes.push_str(&format!(
                    "{} is deleted by the diff; what still uses it is not checked here\n",
                    gone.strip_prefix(workspace_root).unwrap_or(gone).display()
                ));
            }
            patched.texts
        } else if let Some(edit) = args.get("workspace_edit") {
            let (planned, moves) = crate::refactor::planned_texts(workspace_root, edit)?;
            if moves {
                notes.push_str(
                "the edit also creates, renames or deletes files; those parts are not checked\n",
            );
            }
            planned
        } else {
            args.get("edits")
                .and_then(|v| v.as_array())
                .context("Missing 'edits' argument (or `diff`, or `workspace_edit`)")?
                .iter()
                .map(|e| {
                    let path = e
                        .get("path")
                        .and_then(|v| v.as_str())
                        .context("edit without 'path'")?;
                    let text = e
                        .get("new_text")
                        .and_then(|v| v.as_str())
                        .with_context(|| format!("edit for {path} without 'new_text'"))?;
                    Ok((resolve_file_path(workspace_root, path), text.to_string()))
                })
                .collect::<Result<_>>()?
        };
    if edits.is_empty() {
        return Ok(McpToolCallResult::error(
            "the change touches no file that can be checked".to_string(),
        ));
    }
    let also_check: Vec<std::path::PathBuf> = args
        .get("also_check")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|p| resolve_file_path(workspace_root, p))
                .collect()
        })
        .unwrap_or_default();
    let reports =
        crate::diagnostics::validate_texts(remote, workspace_root, &edits, &also_check).await?;
    let errors: usize = reports.iter().map(|r| r.errors).sum();
    let warnings: usize = reports.iter().map(|r| r.warnings).sum();
    let excluded: usize = reports
        .iter()
        .filter(|r| r.is_platform_excluded().is_some())
        .count();
    let active = reports.len().saturating_sub(excluded);
    let mut text = if excluded > 0 {
        format!(
            "{} file(s) checked ({} active, {} platform-excluded): {errors} error(s), {warnings} warning(s)\n{notes}",
            reports.len(),
            active,
            excluded
        )
    } else {
        format!(
            "{} file(s) checked together: {errors} error(s), {warnings} warning(s)\n{notes}",
            reports.len()
        )
    };
    for report in &reports {
        text.push_str(&report.render());
    }
    let mut errors = errors;
    let borrow_check = args.get("borrow_check").and_then(|v| v.as_bool()) == Some(true);
    let compile = args.get("compile").and_then(|v| v.as_bool()) == Some(true);
    if compile || borrow_check {
        let (compiled_errors, compiled) = compile_check(remote, workspace_root, &edits).await?;
        text.push_str(&format!("\n{compiled}"));
        errors += compiled_errors;
    }
    let text = text.trim_end().to_string();
    Ok(if errors == 0 {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub mod diagnostics_handler;
pub use diagnostics_handler::*;

pub(crate) mod call_handlers;
pub(crate) use call_handlers::*;

pub(crate) mod definition;
pub(crate) use definition::*;

pub(crate) mod exec_handler;
pub(crate) use exec_handler::*;

pub(crate) mod editor_handlers;
pub(crate) use editor_handlers::*;

async fn handle_symbols(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .context("Missing 'query' argument")?;
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(30) as usize;
    let hint = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    // More than are shown: a server lists its fuzzy matches in its own order, and the names
    // that hold the query may come after the limit.
    let mut hits = symbol_search_across_projects(
        remote,
        workspace_root,
        query,
        hint.as_deref(),
        limit.max(SYMBOL_CANDIDATES),
    )
    .await?;
    if hits.is_empty() {
        let unindexed = unindexed_declarations(remote, workspace_root, query).await;
        return Ok(McpToolCallResult::text(format!(
            "No symbols match `{query}`.{unindexed}"
        )));
    }
    Ok(McpToolCallResult::text(render_symbol_hits(
        workspace_root,
        query,
        &mut hits,
        limit,
    )))
}

/// How many hits `code_symbols` asks the server for before ranking them.
const SYMBOL_CANDIDATES: usize = 100;

/// The hits of a name search, best matches first (#326). Names that only have the query's
/// letters in order are shown when there is nothing better, and said to be that.
fn render_symbol_hits(root: &Path, query: &str, hits: &mut Vec<SymbolHit>, limit: usize) -> String {
    hits.sort_by_key(|hit| match_rank(&hit.name, query));
    let fuzzy = hits
        .iter()
        .filter(|hit| match_rank(&hit.name, query) == 4)
        .count();
    let all_fuzzy = fuzzy == hits.len();
    let mut out = if all_fuzzy {
        format!(
            "No symbol is named like `{query}`; {} whose names have its letters in order:\n",
            hits.len().min(limit)
        )
    } else {
        hits.retain(|hit| match_rank(&hit.name, query) < 4);
        format!("{} symbol(s) matching `{query}`:\n", hits.len().min(limit))
    };
    for hit in hits.iter().take(limit) {
        out.push_str(&format!("  {}\n", hit.render(root)));
    }
    if fuzzy > 0 && !all_fuzzy {
        out.push_str(&format!(
            "  ({fuzzy} more only have its letters in order; not shown)\n"
        ));
    }
    out.trim_end().to_string()
}

async fn handle_migrate_type(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .or_else(|| args.get("file"))
        .and_then(|v| v.as_str());
    let symbol = args.get("symbol").and_then(|v| v.as_str());
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .or_else(|| args.get("col"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument: the type it should become")?;
    let convert = args
        .get("convert")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let transitive = args
        .get("transitive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

    let file_path = if let Some(p) = path_str {
        resolve_file_path(workspace_root, p)
    } else if let Some(sym) = symbol {
        let clean_sym = sym
            .rsplit("::")
            .next()
            .unwrap_or(sym)
            .rsplit('.')
            .next()
            .unwrap_or(sym)
            .trim();
        let mut found = None;
        for entry in ignore::WalkBuilder::new(workspace_root).build().flatten() {
            let p = entry.path();
            if p.is_file()
                && let Ok(content) = std::fs::read_to_string(p)
                && content.contains(clean_sym)
            {
                found = Some(p.to_path_buf());
                break;
            }
        }
        found.with_context(|| format!("could not find file declaring symbol `{sym}`"))?
    } else {
        anyhow::bail!("Missing 'path' or 'symbol' argument");
    };

    let done = crate::type_migration::migrate_ext(
        remote,
        workspace_root,
        &file_path,
        symbol,
        line,
        character,
        to,
        convert,
        transitive,
        apply,
        force,
    )
    .await?;
    let clean = done.sites.is_empty();
    let text = done.render(40);
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) fn resolve_file_path(workspace_root: &Path, path_str: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(path_str);
    if p.is_absolute() {
        p
    } else {
        workspace_root.join(p)
    }
}

/// Helper to connect, initialize, and execute a targeted LSP request against the remote gateway.
pub async fn execute_lsp_query(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    // One long-lived session per checkout for the life of this process (see
    // crate::session::pooled_query): local edits are pushed before the query.
    crate::session::pooled_query(remote, workspace_root, file_path, method, params).await
}

/// Refuses to write a plan that left references it did not rewrite. With `verify: "compile"` the
/// planner runs as a dry run and the compile gate writes its files, so its own refusal never
/// comes; the gate judges compilation, and neither it nor `force` completes a plan (#446).
pub(crate) fn refuse_incomplete(apply: bool, unmatched: &[String]) -> Result<()> {
    anyhow::ensure!(
        !apply || unmatched.is_empty(),
        "{} reference(s) were not rewritten; nothing was written:\n  {}",
        unmatched.len(),
        unmatched.join("\n  ")
    );
    Ok(())
}

/// What asking the compiler added to a write tool's run.
pub(crate) struct CompileGate {
    pub(crate) text: String,
    pub(crate) passed: bool,
    pub(crate) applied: bool,
}

/// `verify: "compile"`. The tool has built its edit without writing it; the compiler judges it in
/// a shadow of the workspace, and only a result both the analyzer and the compiler accept is
/// written. The overlay check alone does not see an unresolved type (#63), which is precisely
/// what a tool that creates or moves a name can produce.
pub(crate) async fn compile_gate(
    remote: SocketAddr,
    root: &Path,
    files: &[(String, String)],
    analyzer_clean: bool,
    apply: bool,
    force: bool,
) -> Result<CompileGate> {
    if !analyzer_clean && !force {
        return Ok(CompileGate {
            text: "\nthe compiler was not asked: the analyzer already rejects the result\n".into(),
            passed: false,
            applied: false,
        });
    }
    let verdict = crate::compile_check::check(remote, root, files).await?;
    let mut text = verdict.render();
    let mut applied = false;
    if apply {
        if verdict.passed || force {
            let files: std::collections::BTreeMap<std::path::PathBuf, String> = files
                .iter()
                .map(|(p, t)| (std::path::PathBuf::from(p), t.clone()))
                .collect();
            crate::refactor::apply_workspace_edit(
                root,
                &crate::signature::whole_file_edit(&files),
            )?;
            applied = true;
        } else {
            text.push_str(
                "\nnothing was written: the compiler rejects it. Pass `force: true` to write it anyway.\n",
            );
        }
    }
    Ok(CompileGate {
        text,
        passed: verdict.passed,
        applied,
    })
}

/// Tools whose `symbol` is the name of the thing to act on rather than a way of pointing at a
/// position. They are not symbol-addressable: nothing resolves their `symbol` to a
/// path/line/character before the handler runs, because the handler wants the name itself.
#[cfg(test)]
const NAMES_A_SYMBOL: &[&str] = &[
    "code_generate_fixture",
    "code_inline_parameter",
    "code_extract_delegate",
    "code_loop_to_iterator",
];

pub mod symbols;
pub use symbols::*;

#[cfg(test)]
mod tests;
