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

pub(crate) fn parse_exec_argv(args: &serde_json::Value) -> Result<Vec<String>> {
    if let Some(arr) = args
        .get("argv")
        .or_else(|| args.get("command"))
        .or_else(|| args.get("cmd"))
        .and_then(|v| v.as_array())
    {
        Ok(arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect())
    } else if let Some(cmd_str) = args
        .get("argv")
        .or_else(|| args.get("command"))
        .or_else(|| args.get("cmd"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Ok(vec![
            "sh".to_string(),
            "-c".to_string(),
            cmd_str.to_string(),
        ])
    } else {
        anyhow::bail!(
            "Missing 'argv' argument (expected `argv: [\"cmd\", \"arg\", ...]` or `command: \"...\"`)"
        )
    }
}

fn parse_exec_env(args: &serde_json::Value) -> Result<Vec<(String, String)>> {
    let mut env =
        std::collections::BTreeMap::from([("CARGO_TERM_COLOR".to_string(), "never".to_string())]);
    match args.get("env") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Object(values)) => {
            for (key, value) in values {
                anyhow::ensure!(
                    !key.is_empty() && !key.contains(['=', '\0']),
                    "env key must be a non-empty environment variable name"
                );
                let value = value
                    .as_str()
                    .with_context(|| format!("env `{key}` must be a string, got {value}"))?;
                anyhow::ensure!(!value.contains('\0'), "env `{key}` contains a null byte");
                env.insert(key.clone(), value.to_string());
            }
        }
        Some(other) => anyhow::bail!("env must be an object of strings, got {other}"),
    }
    Ok(env.into_iter().collect())
}

pub(crate) async fn handle_exec(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    run_exec_command(remote, workspace_root, args, true).await
}

async fn run_exec_command(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
    pull_back: bool,
) -> Result<McpToolCallResult> {
    let argv = parse_exec_argv(args)?;
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
    let env = parse_exec_env(args)?;
    let outcome = crate::exec::run_remote(
        remote,
        workspace_root,
        subdir.as_deref(),
        argv.clone(),
        env,
        timeout_secs,
        pull_back,
        |_, data| tail.push(data),
    )
    .await?;
    let warning = outcome.platform_warning(workspace_root);
    let exit = outcome.exit;
    if exit.error.is_none() {
        crate::exec::remember_exec_node(workspace_root, remote);
    }
    let status = match (&exit.error, exit.timed_out, exit.exit_code) {
        (Some(err), _, _) => format!("failed to start: {err}"),
        (None, true, _) => "timed out".to_string(),
        (None, false, Some(code)) => format!("exit code {code}"),
        (None, false, None) => "killed by signal".to_string(),
    };
    let mut text = format!(
        "$ {}\n[{status} in {:.1}s{} on node {remote}, copy {}{}; {} bytes of output{}]\n",
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
            "[{} file(s) with changed or unverified local state were kept as they are; the node's version was not written: {}]\n",
            outcome.kept_files.len(),
            outcome.kept_files.join(", ")
        ));
    }
    if let Some(warning) = warning {
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

const DEFAULT_COVERAGE_TOOLCHAIN: &str = "1.97.1";

pub(crate) async fn handle_coverage(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let workspace = args
        .get("workspace")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let package = args.get("package").and_then(serde_json::Value::as_str);
    anyhow::ensure!(
        !workspace || package.is_none(),
        "choose either a package or `workspace: true`, not both"
    );
    let mut argv = vec!["cargo".to_string(), "llvm-cov".to_string()];
    if workspace {
        argv.push("--workspace".to_string());
    } else if let Some(package) = package {
        anyhow::ensure!(!package.trim().is_empty(), "package must not be empty");
        argv.extend(["-p".to_string(), package.to_string()]);
    }
    let extra_args = match args.get("args") {
        None => vec!["--no-report".to_string(), "--lib".to_string()],
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .with_context(|| format!("coverage arg must be a string, got {value}"))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(other) => anyhow::bail!("args must be an array of strings, got {other}"),
    };
    argv.extend(extra_args);
    let toolchain = args
        .get("toolchain")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(DEFAULT_COVERAGE_TOOLCHAIN);
    anyhow::ensure!(
        !toolchain.trim().is_empty() && !toolchain.contains('\0'),
        "toolchain must be a non-empty Rust toolchain name"
    );
    let exec_args = serde_json::json!({
        "argv": argv,
        "timeout_secs": args.get("timeout_secs").and_then(serde_json::Value::as_u64).unwrap_or(3600),
        "tail_bytes": args.get("tail_bytes").and_then(serde_json::Value::as_u64).unwrap_or(16 * 1024),
        "env": { "RUSTUP_TOOLCHAIN": toolchain },
    });
    run_exec_command(remote, workspace_root, &exec_args, false).await
}
