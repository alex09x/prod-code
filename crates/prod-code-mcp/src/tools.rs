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

pub(crate) mod batch;
mod compiler_proposal;
pub mod execution;
mod validation_diff;
mod validation_proposal;
mod workspace_writer;
pub use execution::*;
pub(crate) use workspace_writer::workspace_exec_lock;
pub(crate) async fn dispatch_tool(
    remote: SocketAddr,
    root: &Path,
    tool_name: &str,
    args: serde_json::Value,
) -> Result<McpToolCallResult> {
    match tool_name {
        "code_symbols" => Box::pin(handle_symbols(remote, root, &args)).await,
        "code_safe_delete" => Box::pin(handle_safe_delete(remote, root, &args)).await,
        "code_assists" | "code_assist" => {
            Box::pin(handle_assists(remote, root, tool_name, &args)).await
        }
        "code_check" | "code_lint" | "code_test" | "code_benchmarks" => {
            Box::pin(handle_check(remote, root, tool_name, &args)).await
        }
        "code_rename" => Box::pin(handle_rename(remote, root, &args)).await,
        "code_exec" => Box::pin(handle_exec(remote, root, &args)).await,
        "code_pull" => Box::pin(pull_handler::handle_pull(remote, root, &args)).await,
        "code_definition" => Box::pin(handle_definition(remote, root, &args)).await,

        "code_callers" | "code_callees" => {
            Box::pin(handle_callers(remote, root, tool_name, &args)).await
        }
        "code_implementations" => Box::pin(handle_implementations(remote, root, &args)).await,
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
            let file_path = resolve_file_path(root, path_str);
            let found = crate::supertypes::supertypes(
                remote,
                root,
                &file_path,
                num("line")?,
                num("character")?,
                depth,
            )
            .await?;
            Ok(McpToolCallResult::text(found.render(root)))
        }
        "code_impact" => {
            let base = args.get("base").and_then(|v| v.as_str());
            let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(4) as usize;
            let report = crate::impact::analyze(remote, root, base, depth).await?;
            Ok(McpToolCallResult::text(report.render()))
        }
        "code_diagnose_failure" => Box::pin(handle_diagnose_failure(remote, root, &args)).await,
        "code_diagnostics" | "code_validate_edit" => {
            Box::pin(handle_diagnostics(remote, root, tool_name, &args)).await
        }
        "code_validate_edits" => Box::pin(handle_validate_edits(remote, root, &args)).await,
        "code_shadow_run" => Box::pin(handle_shadow_run(remote, root, &args)).await,
        "code_slice" => Box::pin(handle_slice(remote, root, &args)).await,
        "code_search" => Box::pin(handle_search(remote, root, &args)).await,
        "code_grep" => Box::pin(handle_grep(remote, root, &args)).await,
        "code_codemod" => Box::pin(handle_codemod(remote, root, &args)).await,
        "code_schema_rename" => Box::pin(handle_schema_rename(remote, root, &args)).await,
        "code_encapsulate_field" => Box::pin(handle_encapsulate_field(remote, root, &args)).await,
        "code_migrate_type" => Box::pin(handle_migrate_type(remote, root, &args)).await,
        "code_extract_field" => Box::pin(handle_extract_field(remote, root, &args)).await,
        "code_wrap_return" => Box::pin(handle_wrap_return(remote, root, &args)).await,
        "code_make_static" => Box::pin(handle_make_static(remote, root, &args)).await,
        "code_inline_parameter" => Box::pin(handle_inline_parameter(remote, root, &args)).await,
        "code_introduce_variable" => Box::pin(handle_introduce_variable(remote, root, &args)).await,
        "code_extract_function" => Box::pin(handle_extract_function(remote, root, &args)).await,
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
            let file_path = resolve_file_path(root, path_str);
            let done = Box::pin(crate::loop_to_iterator::loop_to_iterator_polyglot(
                remote, root, &file_path, symbol, line, character, apply, force,
            ))
            .await?;
            let text = done.render();
            Ok(if done.diagnostics.is_empty() {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            })
        }
        "code_extract_trait" => Box::pin(handle_extract_trait(remote, root, &args)).await,
        "code_replace_constructor_with_factory" => {
            Box::pin(handle_replace_constructor_with_factory(remote, root, &args)).await
        }
        "code_replace_constructor_with_builder" => {
            Box::pin(handle_replace_constructor_with_builder(remote, root, &args)).await
        }
        "code_replace_constructor" => {
            let mode = args
                .get("mode")
                .and_then(|m| m.as_str())
                .unwrap_or("factory");
            if mode == "builder" {
                Box::pin(handle_replace_constructor_with_builder(remote, root, &args)).await
            } else {
                Box::pin(handle_replace_constructor_with_factory(remote, root, &args)).await
            }
        }
        "code_pull_up" => Box::pin(handle_pull_up(remote, root, &args)).await,
        "code_push_down" => Box::pin(handle_push_down(remote, root, &args)).await,
        "code_replace_inheritance_with_delegation" | "code_replace_inheritance" => {
            Box::pin(handle_replace_inheritance_with_delegation(
                remote, root, &args,
            ))
            .await
        }
        "code_replace_conditional_with_polymorphism" | "code_replace_conditional" => {
            Box::pin(handle_replace_conditional_with_polymorphism(
                remote, root, &args,
            ))
            .await
        }
        "code_extract_interface" => Box::pin(handle_extract_interface(remote, root, &args)).await,
        "code_extract_delegate" => Box::pin(handle_extract_delegate(remote, root, &args)).await,
        "code_convert_to_method" => Box::pin(handle_convert_to_method(remote, root, &args)).await,
        "code_invert_boolean" => Box::pin(handle_invert_boolean(remote, root, &args)).await,
        "code_generify" => Box::pin(handle_generify(remote, root, &args)).await,
        "code_extract_parameter" => Box::pin(handle_extract_parameter(remote, root, &args)).await,
        "code_introduce_parameter_object" => {
            Box::pin(handle_introduce_parameter_object(remote, root, &args)).await
        }
        "code_move" => Box::pin(handle_move(remote, root, &args)).await,
        "code_move_module" => Box::pin(handle_move_module(remote, root, &args)).await,
        "code_move_method" => Box::pin(handle_move_method(remote, root, &args)).await,
        "code_change_signature" => Box::pin(handle_change_signature(remote, root, &args)).await,
        "code_generate_fixture" => Box::pin(handle_generate_fixture(remote, root, &args)).await,
        "code_dead_code" => Box::pin(handle_dead_code(remote, root, &args)).await,
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
            let pruned = Box::pin(crate::prune::prune_orphans_opts(
                remote,
                root,
                crate::dead_code::DeadCodeOptions {
                    include_exported: false,
                    max_files,
                    reachability,
                },
                apply,
                force,
                git_patch,
                commit,
            ))
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
        "code_source" => Box::pin(handle_source(remote, root, &args)).await,
        "code_references" => Box::pin(handle_references(remote, root, &args)).await,

        "code_outline" => Box::pin(handle_outline(remote, root, &args)).await,

        "code_hover" | "code_type_at" => Box::pin(handle_hover(remote, root, &args)).await,

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
                    root,
                    title: text("title"),
                    body: text("body"),
                    force: flag("force"),
                    dry_run: flag("dry_run"),
                    allow_private_names: flag("allow_private_names"),
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

        "code_sync" => handle_sync(remote, root, args).await,

        "code_dependencies" => handle_dependencies(root, &args).await,
        "code_find_duplicates" => handle_find_duplicates(root, &args).await,
        "code_structural_search" => handle_structural_search(root, &args).await,
        "code_propose_expression" => handle_propose_expression(root, &args).await,

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
pub(crate) mod pull_handler;
pub(crate) use exec_handler::*;

pub(crate) mod editor_handlers;
pub(crate) use editor_handlers::*;

pub(crate) mod symbol_handlers;
pub(crate) use symbol_handlers::*;
pub mod compile_gate;
pub use compile_gate::*;

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
