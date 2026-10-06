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
use std::path::{Path, PathBuf};
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

fn checked_position_argument(value: &serde_json::Value, name: &str) -> Result<u32> {
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

async fn handle_implementations(
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
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/implementation",
        params,
    )
    .await?;
    let arr = match &res {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(_) => vec![res.clone()],
        _ => Vec::new(),
    };
    if arr.is_empty() {
        return Ok(McpToolCallResult::text(
            "No implementations found.".to_string(),
        ));
    }
    let mut out = format!("Found {} implementation(s):\n", arr.len());
    for loc in &arr {
        let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let start = loc.get("range").and_then(|r| r.get("start"));
        let l = start
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0)
            + 1;
        let c = start
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0)
            + 1;
        let mut snippet = String::new();
        let target_path = url::Url::parse(uri)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .unwrap_or_else(|| PathBuf::from(crate::remote_fs::uri_to_path(uri)));
        let content_opt = std::fs::read_to_string(&target_path).ok();
        if let Some(content) = content_opt {
            if let Some(line_str) = content.lines().nth(l.saturating_sub(1) as usize) {
                let trimmed = line_str.trim();
                if !trimmed.is_empty() {
                    snippet = format!("  `{trimmed}`");
                }
            }
        } else if let Ok((bytes, _)) =
            crate::remote_fs::read_source(remote, workspace_root, &target_path.to_string_lossy())
                .await
        {
            let content = String::from_utf8_lossy(&bytes);
            if let Some(line_str) = content.lines().nth(l.saturating_sub(1) as usize) {
                let trimmed = line_str.trim();
                if !trimmed.is_empty() {
                    snippet = format!("  `{trimmed}`");
                }
            }
        }
        out.push_str(&format!("  • {uri}:{l}:{c}{snippet}\n"));
    }
    Ok(McpToolCallResult::text(out.trim_end().to_string()))
}

async fn handle_callers(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
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
    let incoming = tool_name == "code_callers";
    let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let file_path = resolve_file_path(workspace_root, path_str);
    let tree = || {
        crate::call_tree::call_tree(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            incoming,
            depth,
        )
    };
    let mut found = tree().await?;
    let mut note = None;
    if incoming
        && found.as_ref().is_some_and(|t| t.nodes.is_empty())
        && let Some((built, text)) = build_swift_index(remote, workspace_root, &file_path).await
    {
        note = Some(text);
        if built {
            found = tree().await?;
        }
    }
    let body = match found {
        Some(tree) => tree.render(),
        None => format!("No function at {path_str}:{line}:{character}."),
    };
    Ok(McpToolCallResult::text(match note {
        Some(note) => format!("{note}\n{body}"),
        None => body,
    }))
}

/// The SwiftPM packages whose index this process built, so a search asks for a build once.
fn swift_indexes_built() -> &'static std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>
{
    static BUILT: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    > = std::sync::OnceLock::new();
    BUILT.get_or_init(Default::default)
}

/// The SwiftPM package a Swift file belongs to: the nearest directory above it, inside `root`,
/// with a `Package.swift`.
fn swift_package_of(root: &Path, file: &Path) -> Option<std::path::PathBuf> {
    if file.extension().and_then(|e| e.to_str()) != Some("swift") {
        return None;
    }
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let file = std::fs::canonicalize(file).ok()?;
    file.ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(&root))
        .find(|dir| dir.join("Package.swift").is_file())
        .map(Path::to_path_buf)
}

/// Builds the index of the SwiftPM package `file` is in, on that package's node, once per
/// process, for a search in it that found nothing. sourcekit-lsp finds a use in another file
/// only through the index store a build leaves (#166); in a package never built on the node it
/// answers with nothing, which reads like "nothing uses this" (#358). The running server picks
/// the new store up. Returns whether the build succeeded and a line saying what was done;
/// `None` for a file in no package, or in one this process built already.
pub(crate) async fn build_swift_index(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
) -> Option<(bool, String)> {
    let package = swift_package_of(root, file)?;
    if !swift_indexes_built()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(package.clone())
    {
        return None;
    }
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let subdir = package
        .strip_prefix(&canonical_root)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| !p.is_empty());
    let package_named = subdir.as_deref().map_or_else(
        || "the package".to_string(),
        |dir| format!("the package in {dir}"),
    );
    let node = crate::cluster::route_for_path(remote, root, package.to_str())
        .await
        .unwrap_or(remote);
    let mut output = Vec::new();
    let outcome = crate::exec::run_remote(
        node,
        root,
        subdir.as_deref(),
        vec![
            "swift".to_string(),
            "build".to_string(),
            "--build-tests".to_string(),
        ],
        Vec::new(),
        900,
        false,
        |_, bytes| {
            output.extend_from_slice(bytes);
            let excess = output.len().saturating_sub(4096);
            output.drain(..excess);
        },
    )
    .await;
    let why = |outcome: String| {
        format!(
            "sourcekit-lsp finds uses in other files only through a build's index, and \
             `swift build --build-tests` for {package_named} {outcome}: uses outside this file \
             may be missing."
        )
    };
    // What the build said went wrong: its last line naming an error, else its last line.
    let text = String::from_utf8_lossy(&output);
    let last_line = text
        .lines()
        .rev()
        .find(|l| l.to_ascii_lowercase().contains("error"))
        .or_else(|| text.lines().rev().find(|l| !l.trim().is_empty()))
        .map(|l| l.trim().chars().take(200).collect::<String>())
        .unwrap_or_default();
    Some(match outcome {
        Ok(o) if o.exit.exit_code == Some(0) && !o.exit.timed_out => (
            true,
            format!(
                "sourcekit-lsp finds uses in other files through a build's index: built \
                 {package_named} first (`swift build --build-tests`, {:.1} s).",
                o.exit.duration_ms as f64 / 1000.0
            ),
        ),
        Ok(o) if o.exit.timed_out => (false, why("timed out".to_string())),
        Ok(o) => (
            false,
            why(format!(
                "failed (exit {}: {last_line})",
                o.exit.exit_code.map_or("?".to_string(), |c| c.to_string())
            )),
        ),
        Err(err) => (false, why(format!("could not run ({err:#})"))),
    })
}

pub(crate) mod definition;
pub(crate) use definition::*;

fn resolve_exec_subdir(workspace_root: &Path, raw_cwd: Option<&str>) -> Result<Option<String>> {
    let Some(raw_cwd) = raw_cwd else {
        return Ok(None);
    };
    if raw_cwd.trim().is_empty() {
        return Ok(None);
    }
    let resolved = resolve_file_path(workspace_root, raw_cwd);
    if !resolved.exists() {
        anyhow::bail!("working directory '{raw_cwd}' does not exist");
    }
    if !resolved.is_dir() {
        anyhow::bail!("working directory '{raw_cwd}' is not a directory");
    }
    let canon_ws =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let canon_resolved = std::fs::canonicalize(&resolved).unwrap_or_else(|_| resolved.clone());
    if canon_resolved == canon_ws {
        return Ok(None);
    }
    let mut curr = canon_resolved.as_path();
    while curr != canon_ws {
        if curr.join(".git").exists() {
            anyhow::bail!(
                "working directory '{raw_cwd}' is inside a nested Git worktree or repository; \
                 nested worktrees cannot be executed through the parent workspace. \
                 Target the worktree directly as its own workspace."
            );
        }
        match curr.parent() {
            Some(parent) => curr = parent,
            None => break,
        }
    }
    let sub = crate::exec::subdir_of(workspace_root, &resolved).with_context(|| {
        format!(
            "working directory '{raw_cwd}' is outside workspace root {}",
            workspace_root.display()
        )
    })?;
    Ok(Some(sub))
}

async fn handle_exec(
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
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let tail_bytes = args
        .get("tail_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(16 * 1024) as usize;
    let mut tail = crate::exec::TailBuffer::new(tail_bytes);
    let subdir = resolve_exec_subdir(workspace_root, args.get("cwd").and_then(|v| v.as_str()))?;
    let outcome = crate::exec::run_remote(
        remote,
        workspace_root,
        subdir.as_deref(),
        argv.clone(),
        vec![("CARGO_TERM_COLOR".to_string(), "never".to_string())],
        timeout_secs,
        true,
        |_, data| tail.push(data),
    )
    .await?;
    let changed_code = outcome.changed_code();
    let exit = outcome.exit;
    let status = match (&exit.error, exit.timed_out, exit.exit_code) {
        (Some(err), _, _) => format!("failed to start: {err}"),
        (None, true, _) => "timed out".to_string(),
        (None, false, Some(code)) => format!("exit code {code}"),
        (None, false, None) => "killed by signal".to_string(),
    };
    let mut text = format!(
        "$ {}\n[{status} in {:.1}s{} on {}{}; {} bytes of output{}]\n",
        argv.join(" "),
        exit.duration_ms as f64 / 1000.0,
        exit.usage
            .map(|u| format!(" ({})", u.render()))
            .unwrap_or_default(),
        exit.server_workspace_root,
        exit.platform
            .as_deref()
            .map(|p| format!(" ({p})"))
            .unwrap_or_default(),
        tail.total,
        if tail.total > tail_bytes {
            ", tail shown"
        } else {
            ""
        }
    );
    if !outcome.pulled_files.is_empty() {
        text.push_str(&format!(
            "[{} file(s) changed by the command were written back: {}]\n",
            outcome.pulled_files.len(),
            outcome.pulled_files.join(", ")
        ));
    }
    if !outcome.kept_files.is_empty() {
        text.push_str(&format!(
            "[{} file(s) changed here while the command ran were kept, and the node's version was not written: {}]\n",
            outcome.kept_files.len(),
            outcome.kept_files.join(", ")
        ));
    }
    if let Some(warning) =
        crate::exec::platform_warning(workspace_root, exit.platform.as_deref(), &changed_code)
    {
        text.push_str(&format!("[{warning}]\n"));
    }
    text.push_str(&tail.text());
    Ok(if matches!(exit.exit_code, Some(0)) {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_rename(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line =
        checked_position_argument(args.get("line").context("Missing 'line' argument")?, "line")?;
    let character = checked_position_argument(
        args.get("character")
            .context("Missing 'character' argument")?,
        "character",
    )?;
    let new_name = args
        .get("new_name")
        .and_then(|v| v.as_str())
        .context("Missing 'new_name' argument")?
        .to_string();
    let file_path = resolve_file_path(workspace_root, path_str);
    if args
        .get("accessors")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        return rename_with_accessors(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            &new_name,
            force,
        )
        .await;
    }
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
        "newName": new_name
    });
    let edit = match execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/rename",
        params,
    )
    .await
    {
        Ok(edit) => edit,
        Err(e) => return Ok(McpToolCallResult::error(format!("rename refused: {e:#}"))),
    };
    if edit.is_null() {
        return Ok(McpToolCallResult::error(
            "rename produced no edits".to_string(),
        ));
    }
    // The analyzer computes the edit; it does not check that the result compiles. A new name
    // that is already declared in the same scope is renamed into a second definition (#98), so
    // the result is checked in the overlay like every other write, and refused if it breaks.
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let (mut planned, moves_files) = crate::refactor::planned_texts(workspace_root, &edit)?;
    // The old name in comments and test names, in every file the rename touches.
    let comments = args
        .get("comments")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut mentioned = crate::rename_mentions::Mentions::default();
    if comments {
        if moves_files {
            return Ok(McpToolCallResult::error(
                "`comments` is not supported with a rename that moves files; rename first, then \
                 run it again at the new name"
                    .to_string(),
            ));
        }
        let text = std::fs::read_to_string(&file_path).unwrap_or_default();
        // A position on no character names no old name; the file's first word is not one.
        let Some(at) = crate::signature::offset_of(&text, line, character) else {
            return Ok(McpToolCallResult::error(format!(
                "{path_str}:{line}:{character} is not a position in the file, so the old name \
                 in comments cannot be found; nothing was written"
            )));
        };
        let start = text[..at]
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        let old: String = text[start..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !planned.iter().any(|(p, _)| *p == file_path) {
            planned.push((file_path.clone(), text.clone()));
        }
        for (_, t) in planned.iter_mut() {
            let (rewritten, found) = crate::rename_mentions::rewrite(t, &old, &new_name);
            mentioned.comments += found.comments;
            mentioned.tests.extend(found.tests);
            *t = rewritten;
        }
    }
    let reports = crate::diagnostics::validate_texts(remote, workspace_root, &planned, &[]).await?;
    let errors: Vec<String> = reports
        .iter()
        .flat_map(|r| {
            r.items
                .iter()
                .filter(|d| d.severity == "error")
                .map(move |d| {
                    format!(
                        "{}{} ({}:{}:{})",
                        d.message.lines().next().unwrap_or(""),
                        d.code
                            .as_deref()
                            .map(|c| format!(" [{c}]"))
                            .unwrap_or_default(),
                        r.file,
                        d.line,
                        d.col
                    )
                })
        })
        .collect();
    if !errors.is_empty() && !force {
        return Ok(McpToolCallResult::error(format!(
            "rename to `{new_name}` refused: the result does not compile ({} error(s)); nothing \
             was written. If `{new_name}` is already declared in that scope, pick another name; \
             pass `force: true` to write it anyway:\n  {}",
            errors.len(),
            errors.join("\n  ")
        )));
    }
    // With `comments` the texts are no longer the analyzer's edit alone: write them whole.
    let touched = if comments {
        let files: std::collections::BTreeMap<std::path::PathBuf, String> = planned
            .into_iter()
            .filter(|(p, t)| std::fs::read_to_string(p).map(|o| o != *t).unwrap_or(true))
            .collect();
        crate::refactor::apply_workspace_edit(
            workspace_root,
            &crate::signature::whole_file_edit(&files),
        )?
    } else {
        crate::refactor::apply_workspace_edit(workspace_root, &edit)?
    };
    let mut text = format!(
        "renamed to `{new_name}`; {} path(s) updated in the checkout:\n{}",
        touched.len(),
        touched.join("\n")
    );
    if comments {
        text.push_str(&format!(
            "\n\nin comments: {} mention(s) of the old name replaced",
            mentioned.comments
        ));
        for (from, to) in &mentioned.tests {
            text.push_str(&format!("\ntest renamed: `{from}` -> `{to}`"));
        }
    }
    if !errors.is_empty() {
        text.push_str(&format!(
            "\n\nwritten with `force`, although the analyzer reports {} error(s):\n  {}",
            errors.len(),
            errors.join("\n  ")
        ));
    }
    if moves_files {
        text.push_str("\n\nthe rename also moved files; that part was not checked before writing");
    }
    Ok(McpToolCallResult::text(text))
}

/// A field renamed together with its accessors (#146): every rename merged into one change per
/// file, checked in one overlay, written only when it compiles unless `force`.
async fn rename_with_accessors(
    remote: SocketAddr,
    workspace_root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    new_name: &str,
    force: bool,
) -> Result<McpToolCallResult> {
    let (merged, renamed) = match crate::rename_accessors::plan(
        remote,
        workspace_root,
        file,
        line,
        character,
        new_name,
    )
    .await
    {
        Ok(plan) => plan,
        Err(e) => return Ok(McpToolCallResult::error(format!("rename refused: {e:#}"))),
    };
    if merged.is_empty() {
        return Ok(McpToolCallResult::error(
            "rename produced no edits".to_string(),
        ));
    }
    let planned: Vec<(std::path::PathBuf, String)> =
        merged.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
    let reports = crate::diagnostics::validate_texts(remote, workspace_root, &planned, &[]).await?;
    let errors: Vec<String> = reports
        .iter()
        .flat_map(|r| {
            r.items
                .iter()
                .filter(|d| d.severity == "error")
                .map(move |d| {
                    format!(
                        "{}{} ({}:{}:{})",
                        d.message.lines().next().unwrap_or(""),
                        d.code
                            .as_deref()
                            .map(|c| format!(" [{c}]"))
                            .unwrap_or_default(),
                        r.file,
                        d.line,
                        d.col
                    )
                })
        })
        .collect();
    if !errors.is_empty() && !force {
        return Ok(McpToolCallResult::error(format!(
            "rename refused: the result does not compile ({} error(s)); nothing was written:\n  {}",
            errors.len(),
            errors.join("\n  ")
        )));
    }
    let touched = crate::refactor::apply_workspace_edit(
        workspace_root,
        &crate::signature::whole_file_edit(&merged),
    )?;
    Ok(McpToolCallResult::text(format!(
        "renamed {}; {} path(s) updated in the checkout:\n{}",
        renamed.join(", "),
        touched.len(),
        touched.join("\n")
    )))
}

async fn handle_check(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let kind = match tool_name {
        "code_check" => crate::verify::VerifyKind::Check,
        "code_lint" => crate::verify::VerifyKind::Lint,
        "code_benchmarks" => crate::verify::VerifyKind::Bench,
        _ => crate::verify::VerifyKind::Test,
    };
    let filter = args
        .get("filter")
        .or_else(|| args.get("test_filter"))
        .or_else(|| args.get("test"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    // `path` selects a nested project (any file or directory inside it).
    let hint = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    let env = match args.get("env") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_string()))
                    .with_context(|| format!("env `{key}` must be a string, got {value}"))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(other) => anyhow::bail!("env must be an object of strings, got {other}"),
    };
    let fix = args.get("fix").and_then(|v| v.as_bool()).unwrap_or(false);
    if fix
        && matches!(
            kind,
            crate::verify::VerifyKind::Check | crate::verify::VerifyKind::Lint
        )
    {
        anyhow::ensure!(
            env.is_empty(),
            "env is not passed to a `fix` run; run without `fix` to set it"
        );
        let fixed = crate::fixit::check_and_fix(
            remote,
            workspace_root,
            hint.as_deref(),
            kind,
            timeout_secs,
        )
        .await?;
        let text = fixed.render(40);
        return Ok(if fixed.ok() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        });
    }
    let report = crate::verify::run_verify_with(
        remote,
        workspace_root,
        hint.as_deref(),
        kind,
        filter.as_deref(),
        timeout_secs,
        &env,
        |_| {},
    )
    .await?;
    let text = report.render(40);
    Ok(if report.ok() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

/// rust-analyzer writes a prelude item an assist introduces by its full path — an extracted
/// function returns `std::prelude::v1::Result<T, anyhow::Error>` in a file that imports
/// `anyhow::Result` (#97). On the lines the assist wrote, the path is dropped and the result
/// checked in the overlay; the shorter spelling is used only when the
/// analyzer accepts it, and rust-analyzer's own otherwise. Returns the edit to apply and how
/// many paths were shortened.
async fn prefer_names_in_scope(
    remote: SocketAddr,
    root: &Path,
    edit: serde_json::Value,
) -> Result<(serde_json::Value, usize)> {
    const PRELUDE: &str = "std::prelude::v1::";
    let (planned, moves_files) = crate::refactor::planned_texts(root, &edit)?;
    if moves_files {
        return Ok((edit, 0));
    }
    let mut shortened = 0usize;
    let mut shorter = Vec::with_capacity(planned.len());
    for (path, text) in planned {
        // Only lines the assist wrote: a line that was already in the file keeps its spelling,
        // whatever it says.
        let before = std::fs::read_to_string(&path).unwrap_or_default();
        let old_lines: std::collections::HashSet<&str> = before.lines().collect();
        let mut out = String::with_capacity(text.len());
        for line in text.split_inclusive('\n') {
            let body = line.trim_end_matches('\n');
            if body.contains(PRELUDE) && !old_lines.contains(body) {
                shortened += body.matches(PRELUDE).count();
                out.push_str(&line.replace(PRELUDE, ""));
            } else {
                out.push_str(line);
            }
        }
        shorter.push((path, out));
    }
    if shortened == 0 {
        return Ok((edit, 0));
    }
    let reports = crate::diagnostics::validate_texts(remote, root, &shorter, &[]).await?;
    if reports.iter().any(|r| r.errors > 0) {
        return Ok((edit, 0));
    }
    let files: std::collections::BTreeMap<std::path::PathBuf, String> =
        shorter.into_iter().collect();
    Ok((crate::signature::whole_file_edit(&files), shortened))
}

async fn handle_assists(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
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
    let (end_line, end_char) = match (
        args.get("end_line").and_then(|v| v.as_u64()),
        args.get("end_character").and_then(|v| v.as_u64()),
    ) {
        (Some(l), Some(c)) => (l as u32, c as u32),
        _ => (line, character),
    };
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let mut params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "range": {
            "start": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
            "end": { "line": end_line.saturating_sub(1), "character": end_char.saturating_sub(1) }
        }
    });
    if tool_name == "code_assist" {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .context("Missing 'id' argument")?;
        params["id"] = serde_json::json!(id);
        if let Some(subtype) = args.get("subtype").and_then(|v| v.as_u64()) {
            params["subtype"] = serde_json::json!(subtype);
        }
        let edit = match execute_lsp_query(
            remote,
            workspace_root,
            &file_path,
            "prodCode/applyAssist",
            params,
        )
        .await
        {
            Ok(edit) => edit,
            Err(e) => {
                return Ok(McpToolCallResult::error(format!("assist refused: {e:#}")));
            }
        };
        let (edit, respelled) = prefer_names_in_scope(remote, workspace_root, edit).await?;
        let touched = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
        let mut text = format!(
            "applied `{id}`; {} path(s) updated in the checkout:\n{}",
            touched.len(),
            touched.join("\n")
        );
        if respelled > 0 {
            text.push_str(&format!(
                "\n\n{respelled} `std::prelude::v1::` path(s) the assist wrote are spelled as the \
                 name already in scope; the analyzer accepts the shorter spelling"
            ));
        }
        return Ok(McpToolCallResult::text(text));
    }
    let list = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "prodCode/assists",
        params,
    )
    .await?;
    let mut out = String::new();
    if let Some(items) = list.as_array() {
        if items.is_empty() {
            out.push_str("no code actions at this position\n");
        }
        for item in items {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
            match item.get("subtype").and_then(|v| v.as_u64()) {
                Some(st) => out.push_str(&format!("{id} (subtype {st}) [{kind}]: {label}\n")),
                None => out.push_str(&format!("{id} [{kind}]: {label}\n")),
            }
        }
    }
    Ok(McpToolCallResult::text(out.trim_end().to_string()))
}

async fn handle_safe_delete(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line =
        checked_position_argument(args.get("line").context("Missing 'line' argument")?, "line")?;
    let character = checked_position_argument(
        args.get("character")
            .context("Missing 'character' argument")?,
        "character",
    )?;
    let file_path = resolve_file_path(workspace_root, path_str);
    // Go has no analyzer safe-delete request. Its narrow compiler-verified planner must run
    // before the Rust parameter scanner can mistake Go syntax for a parameter.
    if file_path
        .extension()
        .is_some_and(|extension| extension == "go")
    {
        return Ok(
            match crate::safe_delete_go::delete_function(
                remote,
                workspace_root,
                &file_path,
                line,
                character,
            )
            .await
            {
                Ok(deleted) => McpToolCallResult::text(format!(
                    "deleted unreferenced Go function {}; compiler-verified under the active Go build flags; 1 path updated:\n{}",
                    deleted.name,
                    deleted.path.display()
                )),
                Err(error) => McpToolCallResult::error(format!("safe delete refused: {error:#}")),
            },
        );
    }
    if file_path
        .extension()
        .is_some_and(|extension| extension == "ts")
    {
        return Ok(
            match crate::safe_delete_typescript::delete_function(
                remote,
                workspace_root,
                &file_path,
                line,
                character,
            )
            .await
            {
                Ok(deleted) => McpToolCallResult::text(format!(
                    "deleted unreferenced private TypeScript function {}; compiler-verified under the active TypeScript configuration; 1 path updated:\n{}",
                    deleted.name,
                    deleted.path.display()
                )),
                Err(error) => McpToolCallResult::error(format!("safe delete refused: {error:#}")),
            },
        );
    }
    // A parameter goes from the declaration and from every call at once, through
    // `change_signature`, which refuses while the body still uses it.
    let text = std::fs::read_to_string(&file_path).unwrap_or_default();
    if let Some((fn_at, name, kept)) = crate::signature::parameter_at(&text, line, character) {
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        // A trait method's parameter goes from the trait, every implementation and every call,
        // by its position (#194).
        if crate::trait_param::owner_of(&text, fn_at).is_some() {
            let (_, open, close) = crate::signature::param_span(&text, fn_at)
                .context("the method has no parameter list")?;
            let (_, declared) = crate::signature::parse_declared(&text[open..close]);
            let index = declared
                .iter()
                .position(|d| d.name == name)
                .context("the parameter is not in the method's list")?;
            let done = crate::trait_param::remove_parameter(
                remote,
                workspace_root,
                &file_path,
                fn_at,
                index,
                true,
                force,
            )
            .await
            .with_context(|| format!("safe delete of the parameter `{name}` refused"))?;
            let text = done.render(6000);
            return Ok(if done.applied && done.diagnostics.is_empty() {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            });
        }
        let request = kept
            .iter()
            .map(|k| crate::signature::parse_param(k))
            .collect::<Result<Vec<_>>>()?;
        let (fl, fc) = crate::signature::position_at(&text, fn_at)?;
        let change = crate::signature::change(
            remote,
            workspace_root,
            &file_path,
            fl,
            fc,
            &request,
            true,
            force,
        )
        .await
        .with_context(|| format!("safe delete of the parameter `{name}` refused"))?;
        let text = format!(
            "the parameter `{name}` is removed, with its argument at every call\n\n{}",
            change.render(6000)
        );
        return Ok(if change.diagnostics.is_empty() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        });
    }
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
    });
    let edit = match execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "prodCode/safeDelete",
        params,
    )
    .await
    {
        Ok(edit) => edit,
        Err(e) => {
            return Ok(McpToolCallResult::error(format!(
                "safe delete refused: {e:#}"
            )));
        }
    };
    let touched = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
    // An answer with no edit is not a deletion: saying "deleted" would be a success that did
    // nothing (#138).
    if touched.is_empty() {
        return Ok(McpToolCallResult::error(
            "safe delete produced no edit; nothing was deleted".to_string(),
        ));
    }
    Ok(McpToolCallResult::text(format!(
        "deleted; {} path(s) updated in the checkout:\n{}",
        touched.len(),
        touched.join("\n")
    )))
}

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
mod tests {
    #[test]
    fn name_scan_source_read_stops_at_the_per_file_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.rs");
        let mut contents = vec![b'x'; super::MAX_NAME_SCAN_FILE_BYTES as usize + 16];
        contents.extend_from_slice(b"target_at_end");
        std::fs::write(&path, contents).unwrap();

        let scanned = super::read_name_scan_text(&path).unwrap();
        assert_eq!(scanned.len(), super::MAX_NAME_SCAN_FILE_BYTES as usize);
        assert!(!super::names_word(&scanned, "target_at_end"));
    }

    #[test]
    fn protobuf_outline_skips_complete_option_and_reserved_statements() {
        let proto = r#"
            syntax = "proto3";
            package example;
            option deprecated = true;
            message Request {
                option deprecated = true;
                reserved 2, 4 to 6;
                extensions 100 to max;
                string name = 1;
            }
            enum State {
                option deprecated = true;
                reserved "OLD";
                READY = 0;
            }
        "#;
        let outline = super::protobuf_outline(
            proto,
            "api.proto",
            &super::OutlineOptions::all(10, false, ""),
        );
        assert!(outline.contains("[Field] name"), "{outline}");
        assert!(outline.contains("[EnumMember] READY"), "{outline}");
        assert!(!outline.contains("deprecated"), "{outline}");
        assert!(!outline.contains("extensions"), "{outline}");
        assert!(!outline.contains("reserved"), "{outline}");
    }

    /// A declaration is the name right after a declaring keyword or a Go receiver; a use, a
    /// path or a local binding is not (#379).
    #[test]
    fn a_declaration_is_the_name_after_a_declaring_keyword() {
        let at = super::declared_at;
        assert_eq!(at("pub struct Lost {", "Lost"), Some(11));
        assert_eq!(at("pub(crate) fn go(x: u8) {}", "go"), Some(14));
        assert_eq!(at("func (s *Server) Serve() error {", "Serve"), Some(17));
        assert_eq!(at("func Serve() {", "Serve"), Some(5));
        assert_eq!(at("    async def fetch(self):", "fetch"), Some(14));
        assert_eq!(at("export function render() {", "render"), Some(16));
        assert_eq!(at("macro_rules! twice {", "twice"), Some(13));
        assert_eq!(at("use crate::Lost;", "Lost"), None);
        assert_eq!(at("    let lost = Lost::new();", "Lost"), None);
        assert_eq!(at("impl Display for Lost {", "Lost"), None);
        assert_eq!(at("pub struct Lostness;", "Lost"), None);
        assert_eq!(
            at("    pub normalized_orders: u64,", "normalized_orders"),
            Some(8)
        );
        assert_eq!(at("    pub(crate) count: usize,", "count"), Some(15));
        assert_eq!(at("    name: String,", "name"), Some(4));
    }

    #[test]
    fn an_item_ends_where_its_brackets_or_its_indentation_do() {
        let rust = [
            "/// Adds.",
            "#[inline]",
            "fn add(a: u8) -> u8 {",
            "    let open = '{'; // a { in a comment",
            "    let s = \"}}\";",
            "    a",
            "}",
            "fn next() {}",
        ];
        assert_eq!(super::item_end(&rust, 2), 6);
        assert_eq!(super::with_leading_docs(&rust, 2), 0);
        assert_eq!(super::item_end(&rust, 7), 7);
        let python = ["def f(x):", "    y = x", "", "    return y", "z = 1"];
        assert_eq!(super::item_end(&python, 0), 3);
        assert_eq!(super::item_end(&["type A = B;", "fn c() {}"], 0), 0);
        let go = ["type T struct {", "\tA int", "\tB string", "}"];
        assert_eq!(super::item_end(&go, 0), 3);
    }

    #[test]
    fn a_body_is_numbered_and_capped() {
        let lines: Vec<String> = (1..=400).map(|n| format!("line {n}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let text = super::numbered_lines(&refs, 9, 12);
        assert_eq!(
            text,
            "10 | line 10\n11 | line 11\n12 | line 12\n13 | line 13"
        );
        // Numbers are right-aligned to the widest.
        assert!(super::numbered_lines(&refs, 7, 10).starts_with(" 8 | line 8"));
        let long = super::numbered_lines(&refs, 0, 399);
        assert!(
            long.ends_with("… 100 more line(s)"),
            "{}",
            &long[long.len() - 40..]
        );
        assert_eq!(long.lines().count(), super::MAX_BODY_LINES + 1);
    }

    #[test]
    fn the_innermost_outline_range_holding_a_position_wins() {
        let outline = serde_json::json!([{
            "name": "Store",
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 20, "character": 1 } },
            "children": [{
                "name": "sum",
                "range": { "start": { "line": 4, "character": 4 }, "end": { "line": 8, "character": 5 } }
            }]
        }, {
            "name": "flat",
            "location": { "range": { "start": { "line": 30, "character": 0 }, "end": { "line": 33, "character": 1 } } }
        }]);
        let mut best = None;
        super::innermost_holding(&outline, (4, 11), &mut best);
        assert_eq!(best, Some(((4, 4), (8, 5))));
        let mut flat = None;
        super::innermost_holding(&outline, (30, 3), &mut flat);
        assert_eq!(flat, Some(((30, 0), (33, 1))));
    }

    #[test]
    fn a_workspace_query_is_anchored_in_the_root_project_not_a_crate_it_leaves_out() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/core\"]\nresolver = \"2\"\n",
        );
        write(
            "crates/core/Cargo.toml",
            "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write("crates/core/src/lib.rs", "pub fn core() {}\n");
        // A crate with a workspace of its own and the shorter path: its own project (#335).
        write(
            "ext/zed/Cargo.toml",
            "[package]\nname = \"zed\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
        );
        write("ext/zed/src/lib.rs", "pub fn ext() {}\n");
        let anchor = super::representative_source_file(root).unwrap();
        assert!(
            anchor.ends_with("crates/core/src/lib.rs"),
            "the anchor is the root workspace's: {}",
            anchor.display()
        );
    }

    use super::*;

    #[test]
    fn rewritten_files_reads_document_changes_and_skips_the_rest() {
        let edit = serde_json::json!({
            "documentChanges": [
                { "kind": "rename", "oldUri": "file:///w/a.rs", "newUri": "file:///w/b.rs" },
                { "textDocument": { "uri": "file:///w/b.rs", "version": null },
                  "edits": [ { "range": {}, "newText": "fn b() {}\n" } ] },
                { "textDocument": { "uri": "file:///w/c.rs" } }
            ]
        });
        assert_eq!(
            rewritten_files(&edit),
            vec![("/w/b.rs".to_string(), "fn b() {}\n".to_string())]
        );
        assert!(rewritten_files(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn codemod_schema_requires_a_rule_and_offers_apply() {
        let tool = list_tools()
            .into_iter()
            .find(|t| t.name == "code_codemod")
            .expect("code_codemod is listed");
        assert_eq!(tool.input_schema["required"], serde_json::json!(["rule"]));
        assert_eq!(tool.input_schema["properties"]["apply"]["type"], "boolean");
        assert!(
            tool.input_schema["properties"]["rule"]["description"]
                .as_str()
                .unwrap()
                .contains("==>>")
        );
    }

    #[test]
    fn shadow_run_schema_takes_hypotheses_and_argv() {
        let tool = list_tools()
            .into_iter()
            .find(|t| t.name == "code_shadow_run")
            .expect("code_shadow_run is listed");
        let required: Vec<&str> = tool.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(required, vec!["hypotheses", "argv"]);
        let item = &tool.input_schema["properties"]["hypotheses"]["items"];
        assert_eq!(item["required"], serde_json::json!(["name"]));
        assert_eq!(
            item["properties"]["edits"]["items"]["required"],
            serde_json::json!(["path", "new_text"])
        );
        assert_eq!(tool.input_schema["properties"]["apply"]["type"], "boolean");
        assert_eq!(
            tool.input_schema["properties"]["in_memory"]["type"],
            "boolean"
        );
        assert_eq!(tool.input_schema["properties"]["ram"]["type"], "boolean");
    }

    #[test]
    fn symbol_addressable_tools_advertise_symbol_and_do_not_require_a_position() {
        for tool in list_tools() {
            let props = tool.input_schema["properties"]
                .as_object()
                .expect("schema has properties");
            let required: Vec<&str> = tool.input_schema["required"]
                .as_array()
                .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            if SYMBOL_ADDRESSABLE.contains(&tool.name.as_str()) {
                assert!(props.contains_key("symbol"), "{} lacks `symbol`", tool.name);
                assert!(props.contains_key("path"), "{} lacks `path`", tool.name);
                for positional in ["path", "line", "character"] {
                    assert!(
                        !required.contains(&positional),
                        "{} still requires `{positional}`",
                        tool.name
                    );
                }
            } else if !NAMES_A_SYMBOL.contains(&tool.name.as_str()) {
                assert!(
                    !props.contains_key("symbol"),
                    "{} unexpectedly takes `symbol`",
                    tool.name
                );
            }
        }
        let rename = list_tools()
            .into_iter()
            .find(|t| t.name == "code_rename")
            .unwrap();
        let required: Vec<&str> = rename.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(required, vec!["new_name"]);
    }

    #[test]
    fn validate_edits_schema_accepts_alternative_inputs() {
        let tool = list_tools()
            .into_iter()
            .find(|t| t.name == "code_validate_edits")
            .unwrap();
        // Each public input form stands alone: a diff or WorkspaceEdit must not need
        // an unrelated, dummy `edits` list to satisfy MCP client validation.
        assert!(tool.input_schema.get("required").is_none());
        assert_eq!(
            tool.input_schema["anyOf"],
            serde_json::json!([
                { "required": ["edits"] },
                { "required": ["diff"] },
                { "required": ["workspace_edit"] }
            ])
        );
        assert_eq!(
            tool.input_schema["properties"]["edits"]["items"]["required"],
            serde_json::json!(["path", "new_text"])
        );
    }

    #[test]
    fn outline_schema_offers_locals_toggle() {
        let outline = list_tools()
            .into_iter()
            .find(|t| t.name == "code_outline")
            .unwrap();
        assert!(outline.input_schema["properties"]["include_locals"].is_object());
    }

    #[test]
    fn qualifier_matches_resolves_go_receiver_and_package_methods() {
        let root = Path::new("/workspace");
        let hit = SymbolHit {
            path: root.join("internal/web/delegation.go"),
            name: "(*Server).verifyRenewablePrimaryTokenRecord".to_string(),
            kind: "Method",
            container: Some("prod/internal/web".to_string()),
            line: 42,
            col: 1,
        };

        // Exact receiver name returned by symbol discovery (#752)
        assert!(super::qualifier_matches(root, &hit, &["Server"]));
        // Pointer-receiver syntax
        assert!(super::qualifier_matches(root, &hit, &["(*Server)"]));
        // Package-qualified receiver
        assert!(super::qualifier_matches(root, &hit, &["web", "Server"]));
        // Fully-qualified module path
        assert!(super::qualifier_matches(
            root,
            &hit,
            &["prod", "internal", "web", "Server"]
        ));

        // Unrelated receiver must not match
        assert!(!super::qualifier_matches(root, &hit, &["Client"]));
        assert!(!super::qualifier_matches(
            root,
            &hit,
            &["otherpkg", "Server"]
        ));

        // Value receiver
        let val_hit = SymbolHit {
            path: root.join("internal/runner/runner.go"),
            name: "(Runner).Run".to_string(),
            kind: "Method",
            container: Some("runner".to_string()),
            line: 10,
            col: 1,
        };
        assert!(super::qualifier_matches(root, &val_hit, &["Runner"]));
        assert!(super::qualifier_matches(
            root,
            &val_hit,
            &["runner", "Runner"]
        ));
        assert!(!super::qualifier_matches(root, &val_hit, &["Server"]));
    }

    #[test]
    fn declared_at_matches_var_let_val_declarations() {
        assert_eq!(
            super::declared_at("    var searchState: State", "searchState"),
            Some(8)
        );
        assert_eq!(
            super::declared_at("@Published var searchState: State", "searchState"),
            Some(15)
        );
        assert_eq!(super::declared_at("let count = 42;", "count"), Some(4));
        assert_eq!(super::declared_at("val items = listOf()", "items"), Some(4));
    }
}
