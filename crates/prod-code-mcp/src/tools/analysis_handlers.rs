/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::Path;

use super::diagnostics_handler::compile_check;
use super::exec_handler::resolve_exec_subdir;
use super::resolve_file_path;
use super::symbols::resolve_symbol;
use crate::protocol::McpToolCallResult;

pub(crate) async fn handle_search(
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

pub(crate) async fn handle_slice(
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

pub(crate) async fn handle_shadow_run(
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

pub(crate) async fn handle_validate_edits(
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
