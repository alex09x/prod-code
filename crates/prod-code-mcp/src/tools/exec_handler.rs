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

use super::resolve_file_path;
use crate::protocol::McpToolCallResult;

pub(crate) fn resolve_exec_subdir(
    workspace_root: &Path,
    raw_cwd: Option<&str>,
) -> Result<Option<String>> {
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

pub(crate) async fn handle_exec(
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
    // Bound unencoded tail bytes to reserve room for worst-case JSON escaping (2x) and
    // the JSON-RPC response envelope within MAX_JSONRPC_FRAME_BYTES (60KB).
    const MAX_EXEC_TAIL_BYTES: usize = 20 * 1024;
    let tail_bytes = (args
        .get("tail_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(16 * 1024) as usize)
        .min(MAX_EXEC_TAIL_BYTES);
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
    Ok(
        if exit.error.is_none() && !exit.timed_out && exit.exit_code.is_some() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        },
    )
}
