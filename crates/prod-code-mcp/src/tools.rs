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
use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::Path;

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

pub(crate) mod inspection_handlers;
pub(crate) use inspection_handlers::*;
pub(crate) mod references;
pub(crate) use references::*;

pub(crate) mod source_handlers;
pub(crate) use source_handlers::*;
pub(crate) mod refactor_handlers;
pub(crate) use refactor_handlers::*;

pub(crate) mod analysis_handlers;
pub(crate) use analysis_handlers::*;

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
