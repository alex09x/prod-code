/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Diagnostic reporting, compiler check integration, and failure diagnosis.

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};

use super::resolve_file_path;
use crate::protocol::McpToolCallResult;

pub(crate) async fn handle_diagnostics(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .or_else(|| args.get("file"))
        .or_else(|| args.get("file_path"))
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (expected `path: \"...\"`)")?;
    let file_path = resolve_file_path(workspace_root, path_str);

    if tool_name == "code_validate_edit" {
        if let Some(chunk) = args.get("chunk").and_then(|v| v.as_str()) {
            let (session_id, implicit_session) =
                match args.get("session_id").and_then(|v| v.as_str()) {
                    Some(id) if !id.trim().is_empty() => (id.to_string(), false),
                    _ => {
                        let counter = crate::diagnostics::next_batch_counter();
                        (format!("stream-{}-{}", std::process::id(), counter), true)
                    }
                };
            let close = args.get("close").and_then(|v| v.as_bool()).unwrap_or(false);
            let reset =
                args.get("reset").and_then(|v| v.as_bool()).unwrap_or(false) || implicit_session;
            let borrow_check = args
                .get("borrow_check")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || args
                    .get("compile")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
            let mgr = crate::diagnostics::stream_manager();
            let chunk_res = mgr
                .feed_chunk(
                    remote,
                    workspace_root,
                    &file_path,
                    &session_id,
                    chunk,
                    close,
                    reset,
                    borrow_check,
                )
                .await?;
            let text = chunk_res.render();
            let ok = !chunk_res.intercepted;
            return Ok(if ok {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            });
        }
        if let Some(chunks) = args.get("stream_chunks").and_then(|v| v.as_array()) {
            let chunk_strs: Vec<String> = chunks
                .iter()
                .filter_map(|c| c.as_str().map(|s| s.to_string()))
                .collect();
            let borrow_check = args
                .get("borrow_check")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || args
                    .get("compile")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
            let stream_res = crate::diagnostics::validate_stream_chunks(
                remote,
                workspace_root,
                &file_path,
                &chunk_strs,
                borrow_check,
            )
            .await?;
            let text = stream_res.render();
            let ok = !stream_res.intercepted;
            return Ok(if ok {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            });
        }
    }

    let proposed_text = args
        .get("new_text")
        .or_else(|| args.get("content"))
        .or_else(|| args.get("text"))
        .and_then(|v| v.as_str());
    let report = match proposed_text {
        Some(text) if tool_name == "code_validate_edit" => {
            crate::diagnostics::validate_text(remote, workspace_root, &file_path, text).await?
        }
        _ if tool_name == "code_validate_edit" => {
            return Ok(McpToolCallResult::error(
                "Missing 'new_text' argument (expected `path` and `new_text` / `content`, or `chunk` / `stream_chunks`)".to_string(),
            ));
        }
        _ => crate::diagnostics::diagnostics(remote, workspace_root, &file_path).await?,
    };
    let mut text = report.render();
    let mut ok = report.ok();
    let borrow_check = args
        .get("borrow_check")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let compile = args
        .get("compile")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if tool_name == "code_validate_edit"
        && (compile || borrow_check)
        && let Some(proposed) = proposed_text
    {
        let (errors, compiled) =
            compile_check(remote, workspace_root, &[(file_path, proposed.to_string())]).await?;
        text.push('\n');
        text.push_str(&compiled);
        ok = ok && errors == 0;
    }
    Ok(if ok {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

/// Runs the checkout's check command (`cargo check` for Rust, `go build`, `tsc`, ...) in a
/// shadow copy on the node that holds the proposed texts, for `compile: true`. The analyzer's
/// overlay misses what only the compiler checks: rust-analyzer runs no borrow checker, so a
/// reference to a local (E0515) or a use after a move (E0382) validated clean (#364). Returns
/// the compiler's error count and a report of them.
pub async fn compile_check(
    remote: SocketAddr,
    root: &Path,
    edits: &[(std::path::PathBuf, String)],
) -> Result<(usize, String)> {
    let proposal = super::validation_proposal::ValidationProposal::from_texts(root, edits)?;
    super::compiler_proposal::compile_proposal(remote, root, &proposal).await
}

/// The findings in a check command's output, by the language's format.
pub(crate) fn compile_diagnostics(language: &str, output: &str) -> Vec<crate::verify::Diagnostic> {
    match language {
        "rust" => {
            let json: Vec<_> = output
                .lines()
                .filter_map(crate::verify::parse_cargo_json_line)
                .collect();
            if json.is_empty() {
                crate::verify::parse_rustc_text(output)
            } else {
                json
            }
        }
        "go" => crate::verify::parse_go_text(output),
        "typescript" => crate::verify::parse_tsc_text(output),
        "python" => crate::verify::parse_pyright_json(output),
        "swift" => crate::verify::parse_swift_text(output),
        _ => crate::verify::parse_colon_diagnostics(output),
    }
}

pub(crate) async fn handle_diagnose_failure(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let hint_str = args
        .get("path")
        .or_else(|| args.get("file_path"))
        .or_else(|| args.get("file"))
        .and_then(|v| v.as_str());
    let hint_path = hint_str.map(Path::new);
    let filter = args.get("filter").and_then(|v| v.as_str());
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let as_json = args.get("json").and_then(|v| v.as_bool()).unwrap_or(false);
    let report =
        crate::dossier::diagnose(remote, workspace_root, hint_path, filter, timeout_secs).await?;
    let text = if as_json {
        serde_json::to_string_pretty(&report)?
    } else {
        report.render()
    };
    Ok(
        if report.tests_failed == 0 && report.build_errors.is_empty() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        },
    )
}
