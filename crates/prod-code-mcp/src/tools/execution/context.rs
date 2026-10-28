/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn resolve_workspace_root(
    configured_root: &Path,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<PathBuf> {
    let value = match args.get("workspace") {
        None | Some(serde_json::Value::Null) => return Ok(configured_root.to_path_buf()),
        Some(value) => value,
    };
    let raw = value
        .as_str()
        .context("workspace must be an absolute path to a worktree")?;
    ensure!(!raw.trim().is_empty(), "workspace path cannot be empty");
    ensure!(
        matches!(tool_name, "code_test" | "code_exec"),
        "workspace is supported only by code_test and code_exec"
    );
    let selected = Path::new(raw);
    ensure!(
        selected.is_absolute(),
        "workspace must be an absolute path to a worktree"
    );
    let selected = std::fs::canonicalize(selected)
        .with_context(|| format!("workspace path '{raw}' cannot be read"))?;
    ensure!(
        selected.is_dir(),
        "workspace path '{raw}' is not a directory"
    );

    let configured = std::fs::canonicalize(configured_root)
        .with_context(|| "configured workspace root cannot be read")?;
    if selected == configured {
        return Ok(selected);
    }
    let (configured_top, configured_common) = git_identity(&configured)?;
    let (selected_top, selected_common) = git_identity(&selected)?;
    ensure!(
        selected_top == selected,
        "workspace must name the root of a Git worktree"
    );
    ensure!(
        configured_common == selected_common,
        "workspace must be a worktree of the same Git repository as the configured workspace"
    );
    ensure!(
        configured_top == configured,
        "configured workspace root is not a Git worktree root"
    );
    Ok(selected)
}

fn git_identity(root: &Path) -> Result<(PathBuf, PathBuf)> {
    let top = git_path(root, &["rev-parse", "--show-toplevel"])?;
    let top = std::fs::canonicalize(top).context("Git worktree root cannot be resolved")?;
    let common = git_path(root, &["rev-parse", "--git-common-dir"])?;
    let common = if common.is_absolute() {
        common
    } else {
        root.join(common)
    };
    let common =
        std::fs::canonicalize(common).context("Git common directory cannot be resolved")?;
    Ok((top, common))
}

fn git_path(root: &Path, args: &[&str]) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| format!("git {} could not start", args.join(" ")))?;
    ensure!(output.status.success(), "workspace must be a Git worktree");
    let value = String::from_utf8(output.stdout).context("Git returned a non-UTF-8 path")?;
    let value = value.trim();
    if value.is_empty() {
        bail!("Git returned an empty worktree path");
    }
    Ok(PathBuf::from(value))
}

/// The path a tool is routed by. A text search reads the synced copy, which every node holding
/// the workspace has: a Markdown file needs no node that serves Markdown, nor a Swift one a Mac
/// (#1558).
pub(crate) fn routed_path<'a>(tool_name: &str, path: Option<&'a str>) -> Option<&'a str> {
    (tool_name != "code_grep").then_some(path).flatten()
}
