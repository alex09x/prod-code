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
use std::path::{Path, PathBuf};

use crate::protocol::McpToolCallResult;

pub(crate) async fn handle_pull(
    remote: SocketAddr,
    root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let files = args
        .get("files")
        .and_then(|v| v.as_array())
        .context("Missing 'files' array")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(PathBuf::from)
                .context("files must contain paths")
        })
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(!files.is_empty(), "files must not be empty");
    let node = match args.get("remote").and_then(|v| v.as_str()) {
        Some(node) => node
            .parse()
            .context("remote must be a node address (HOST:PORT)")?,
        None => crate::exec::last_exec_node(root).unwrap_or(remote),
    };
    if args.get("list").and_then(|v| v.as_bool()) == Some(true) {
        anyhow::ensure!(
            files.len() == 1,
            "code_pull list requires exactly one directory"
        );
        let outcome = crate::sync::list_remote_artifacts_from(node, root, &files[0]).await?;
        return Ok(McpToolCallResult::text(outcome.render()));
    }
    let outcome = crate::sync::pull_remote_files_from(node, root, &files).await?;
    Ok(McpToolCallResult::text(format!(
        "Pulled {} file(s) from node {}, copy {}:\n{}",
        outcome.files.len(),
        outcome.node,
        outcome.server_workspace_root,
        outcome.files.join("\n")
    )))
}
