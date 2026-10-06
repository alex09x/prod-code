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
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
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

    let report = match args.get("new_text").and_then(|v| v.as_str()) {
        Some(text) if tool_name == "code_validate_edit" => {
            crate::diagnostics::validate_text(remote, workspace_root, &file_path, text).await?
        }
        _ if tool_name == "code_validate_edit" => {
            return Ok(McpToolCallResult::error(
                "Missing 'new_text' argument".to_string(),
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
        && let Some(proposed) = args.get("new_text").and_then(|v| v.as_str())
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
    let hint = edits.first().map(|(p, _)| p.as_path()).unwrap_or(root);
    let (subdir, engine) = crate::sync::engine_project(root, hint);
    let language = engine
        .or_else(|| crate::sync::expected_engine(root))
        .context("`compile` needs a project manifest")?;
    let project_dir = match &subdir {
        Some(sub) => root.join(sub),
        None => root.to_path_buf(),
    };
    let command = crate::verify::plan_command_with(
        &crate::verify::detect_tools(&project_dir),
        language,
        crate::verify::VerifyKind::Check,
        None,
    )?;
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let spec = crate::shadow::HypothesisSpec {
        name: "proposed".to_string(),
        edits: edits
            .iter()
            .map(|(path, text)| crate::shadow::HypothesisEdit {
                relative_path: path
                    .strip_prefix(&canonical)
                    .or_else(|_| path.strip_prefix(root))
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/"),
                text: Some(text.clone()),
            })
            .collect(),
    };
    let outcome = crate::shadow::run_shadow(
        remote,
        root,
        subdir.as_deref(),
        &[spec],
        command.clone(),
        Vec::new(),
        900,
        1,
        1 << 20,
        false,
    )
    .await?;
    let result = outcome
        .results
        .first()
        .context("the shadow run returned no result")?;
    let run = format!("`{}` on the proposed text", command.join(" "));
    if let Some(error) = &result.error {
        anyhow::bail!("{run} could not run: {error}");
    }
    if result.timed_out {
        anyhow::bail!("{run} timed out");
    }
    // `--all-targets` builds a crate's library and its tests: the same error comes twice.
    let mut seen = std::collections::HashSet::new();
    let errors: Vec<crate::verify::Diagnostic> = compile_diagnostics(language, &result.output)
        .into_iter()
        .filter(|d| d.level == "error" && seen.insert(d.render()))
        .collect();
    let mut report = if errors.is_empty() && result.exit_code != Some(0) {
        // A failure whose output names no error: say so rather than claim it is clean.
        format!(
            "compiler: {run} exited {}; no error in its output could be read:\n{}",
            result.exit_code.map_or("?".to_string(), |c| c.to_string()),
            result
                .output
                .lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        )
    } else {
        format!("compiler: {run}: {} error(s)", errors.len())
    };
    let has_borrow_violation = errors.iter().any(|d| {
        d.code
            .as_deref()
            .map(crate::diagnostics::is_borrow_checker_error_code)
            .unwrap_or(false)
            || d.message.contains("borrow")
            || d.message.contains("moved value")
            || d.message.contains("lifetime")
            || d.message.contains("cannot borrow")
            || d.message.contains("cannot move out")
    });
    if has_borrow_violation {
        report.push_str("\n  [INTERCEPT: BORROW CHECKER VIOLATION detected in proposed edits]");
    }
    for diagnostic in errors.iter().take(20) {
        report.push_str(&format!("\n  {}", diagnostic.render()));
    }
    if errors.len() > 20 {
        report.push_str(&format!("\n  … {} more", errors.len() - 20));
    }
    if errors.is_empty() && result.exit_code == Some(0) {
        report.push_str(" (full compiler & borrow-checker proof verified clean)");
    }
    let count = if errors.is_empty() && result.exit_code != Some(0) {
        1
    } else {
        errors.len()
    };
    Ok((count, report))
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
