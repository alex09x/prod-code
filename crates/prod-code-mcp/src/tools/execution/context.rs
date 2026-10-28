/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::compile_gate::resolve_file_path;
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn resolve_workspace_root(
    configured_root: &Path,
    tool_name: &str,
    args: &mut serde_json::Value,
) -> Result<PathBuf> {
    let supported = matches!(tool_name, "code_test" | "code_exec");
    let explicit = match args.get("workspace") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .context("workspace must be an absolute path to a worktree")?
                .to_string(),
        ),
    };
    let configured = std::fs::canonicalize(configured_root)
        .with_context(|| "configured workspace root cannot be read")?;
    if explicit.is_some() {
        ensure!(
            supported,
            "workspace is supported only by code_test and code_exec"
        );
    }
    let has_explicit_workspace = explicit.is_some();
    let hint = workspace_hint(tool_name, args);
    let selected = if let Some(raw) = explicit {
        explicit_workspace_root(&configured, &raw)?
    } else if let Some((_, raw)) = &hint {
        infer_workspace_from_hint(&configured, raw)?
    } else {
        configured.clone()
    };
    let rewritten_hint = if !has_explicit_workspace
        && selected != configured
        && let Some((key, raw)) = hint.as_ref()
        && !Path::new(raw).is_absolute()
    {
        let resolved = resolve_file_path(&configured, raw);
        if let Ok(path) = std::fs::canonicalize(resolved)
            && let Ok(relative) = path.strip_prefix(&selected)
        {
            Some((key.clone(), relative.to_string_lossy().into_owned()))
        } else {
            None
        }
    } else {
        None
    };
    if let Some((key, path)) = rewritten_hint
        && let Some(object) = args.as_object_mut()
    {
        object.insert(key, serde_json::Value::String(path));
    }
    validate_workspace_hint(&selected, tool_name, args)?;
    Ok(selected)
}

fn explicit_workspace_root(configured: &Path, raw: &str) -> Result<PathBuf> {
    ensure!(!raw.trim().is_empty(), "workspace path cannot be empty");
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
    if selected == configured {
        return Ok(selected);
    }
    let (configured_top, configured_common) = git_identity(configured)?;
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

fn workspace_hint(tool_name: &str, args: &serde_json::Value) -> Option<(String, String)> {
    let keys: &[&str] = if tool_name == "code_test" {
        &["path", "file_path", "file", "package", "crate"]
    } else if tool_name == "code_exec" {
        &["cwd"]
    } else {
        return None;
    };
    let key = keys.iter().find(|key| args.get(**key).is_some())?;
    let raw = args.get(*key)?.as_str()?.trim();
    (!raw.is_empty()).then(|| ((*key).to_string(), raw.to_string()))
}

fn infer_workspace_from_hint(configured: &Path, raw: &str) -> Result<PathBuf> {
    let candidate = resolve_file_path(configured, raw);
    if !candidate.exists() {
        if Path::new(raw).is_absolute() && !candidate.starts_with(configured) {
            bail!(
                "path or cwd '{raw}' is outside workspace root {configured_display}",
                configured_display = configured.display()
            );
        }
        return Ok(configured.to_path_buf());
    }
    let candidate = std::fs::canonicalize(candidate)
        .with_context(|| format!("path or cwd '{raw}' cannot be read"))?;
    let directory = if candidate.is_dir() {
        candidate.as_path()
    } else {
        candidate.parent().context("path has no parent directory")?
    };
    match git_identity(directory) {
        Ok((top, common)) => {
            if top == configured {
                return Ok(configured.to_path_buf());
            }
            let (_, configured_common) = git_identity(configured)?;
            ensure!(
                configured_common == common,
                "path or cwd '{raw}' is outside workspace root {configured_display}",
                configured_display = configured.display()
            );
            Ok(top)
        }
        Err(_) if candidate.starts_with(configured) => {
            ensure!(
                !nested_git_marker(configured, &candidate),
                "path or cwd '{raw}' is inside a nested Git worktree or repository; target that checkout directly"
            );
            Ok(configured.to_path_buf())
        }
        Err(_) => bail!(
            "path or cwd '{raw}' is outside workspace root {configured_display}",
            configured_display = configured.display()
        ),
    }
}

fn validate_workspace_hint(root: &Path, tool_name: &str, args: &serde_json::Value) -> Result<()> {
    let Some((_, raw)) = workspace_hint(tool_name, args) else {
        return Ok(());
    };
    let resolved = resolve_file_path(root, &raw);
    if !resolved.exists() {
        ensure!(
            !Path::new(&raw).is_absolute() || resolved.starts_with(root),
            "path or cwd '{raw}' is outside the selected workspace"
        );
        return Ok(());
    }
    let resolved = std::fs::canonicalize(resolved)
        .with_context(|| format!("path or cwd '{raw}' cannot be read"))?;
    ensure!(
        resolved.starts_with(root),
        "path or cwd '{raw}' is outside the selected workspace"
    );
    let directory = if resolved.is_dir() {
        resolved.as_path()
    } else {
        resolved.parent().context("path has no parent directory")?
    };
    match git_identity(directory) {
        Ok((top, _common)) if top != root => bail!(
            "path or cwd '{raw}' lies inside a nested Git worktree or repository; select that checkout as `workspace`"
        ),
        Err(_) if nested_git_marker(root, &resolved) => bail!(
            "path or cwd '{raw}' is inside a nested Git worktree or repository; select that checkout directly"
        ),
        _ => {}
    }
    Ok(())
}

fn nested_git_marker(root: &Path, path: &Path) -> bool {
    let mut directory = if path.is_dir() {
        path
    } else if let Some(parent) = path.parent() {
        parent
    } else {
        return false;
    };
    while directory.starts_with(root) && directory != root {
        if directory.join(".git").exists() {
            return true;
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        directory = parent;
    }
    false
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
