/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};

use crate::protocol::McpToolCallResult;
use crate::tools::resolve_file_path;

pub(crate) async fn handle_check(
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
