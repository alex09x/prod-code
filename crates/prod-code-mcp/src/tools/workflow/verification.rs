/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::verify::{VerifyKind, plan::scoped::plan_narrow};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyArgs {
    kind: String,
    path: String,
    filter: Option<String>,
    timeout_secs: Option<u64>,
}

/// Validate every literal before any workflow step can write; references are checked
/// again after resolution. Kind stays literal so intent is visible in the plan.
pub(super) fn validate(args: &Value) -> Result<()> {
    let object = args.as_object().context("verify args must be an object")?;
    ensure!(
        object
            .keys()
            .all(|key| matches!(key.as_str(), "kind" | "path" | "filter" | "timeout_secs")),
        "unsupported verify argument"
    );
    let kind = args.get("kind").and_then(Value::as_str);
    ensure!(
        matches!(kind, Some("check" | "lint" | "test")),
        "verify requires literal kind check, lint or test"
    );
    let path = args
        .get("path")
        .context("verify requires a project or file path")?;
    if !super::references::is_reference(path) {
        relative_scope(
            path.as_str()
                .context("verify path must be a string or reference")?,
        )?;
    }
    if kind == Some("test") {
        let filter = args
            .get("filter")
            .context("test verification requires a narrow filter")?;
        if !super::references::is_reference(filter) {
            ensure!(
                filter
                    .as_str()
                    .is_some_and(|value| !value.trim().is_empty()),
                "test filter must be a non-empty string or reference"
            );
        }
    } else {
        ensure!(
            !object.contains_key("filter"),
            "filter is supported only for test verification"
        );
    }
    if let Some(timeout) = args.get("timeout_secs")
        && !super::references::is_reference(timeout)
    {
        ensure!(
            timeout
                .as_u64()
                .is_some_and(|value| (1..=120).contains(&value)),
            "verification timeout_secs must be 1..=120"
        );
    }
    Ok(())
}

fn relative_scope(raw: &str) -> Result<PathBuf> {
    let path = Path::new(raw);
    ensure!(
        !raw.trim().is_empty() && !matches!(raw.trim(), "." | "./" | "./."),
        "verify requires a scope below the workspace root"
    );
    let mut relative = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(name) => relative.push(name),
            Component::CurDir => {}
            _ => bail!("verify path must be workspace-relative without parent segments"),
        }
    }
    ensure!(
        !relative.as_os_str().is_empty(),
        "verify cannot target the workspace root"
    );
    Ok(relative)
}

fn kind(value: &str) -> VerifyKind {
    match value {
        "check" => VerifyKind::Check,
        "lint" => VerifyKind::Lint,
        "test" => VerifyKind::Test,
        _ => unreachable!("validated verify kind"),
    }
}

fn existing_scope(
    root: &Path,
    raw: &str,
    kind: VerifyKind,
    filter: Option<&str>,
) -> Result<String> {
    let relative = relative_scope(raw)?;
    let root = std::fs::canonicalize(root)?;
    let target = std::fs::canonicalize(root.join(relative))
        .context("verify scope must exist before execution")?;
    ensure!(
        target != root && target.starts_with(&root),
        "verify scope must remain below the workspace root"
    );
    let metadata = std::fs::metadata(&target)?;
    ensure!(
        metadata.is_file() || metadata.is_dir(),
        "verify scope must be a regular file or directory"
    );
    let normalized = target
        .strip_prefix(&root)?
        .to_str()
        .context("verify path must be UTF-8")?
        .to_owned();
    let mut selection = json!({"workspace":root,"path":normalized});
    super::resolve_workspace_root(&root, "code_definition", &mut selection)?;
    plan_narrow(&root, Some(&target), kind, filter)?;
    Ok(normalized)
}

pub(super) fn preflight(plan: &super::plan::Plan, root: &Path) -> Result<()> {
    for step in &plan.steps {
        if step.tool == "verify"
            && let Some(path) = step.args.get("path").and_then(Value::as_str)
        {
            let filter = step
                .args
                .get("filter")
                .map(|value| value.as_str().unwrap_or("workflow_preflight_filter"));
            existing_scope(
                root,
                path,
                kind(step.args["kind"].as_str().expect("validated kind")),
                filter,
            )?;
        }
    }
    Ok(())
}

pub(super) fn request(root: &Path, args: Value) -> Result<(&'static str, Value)> {
    validate(&args)?;
    let request: VerifyArgs = serde_json::from_value(args)?;
    let path = existing_scope(
        root,
        &request.path,
        kind(&request.kind),
        request.filter.as_deref(),
    )?;
    let tool = match request.kind.as_str() {
        "check" => "code_check",
        "lint" => "code_lint",
        "test" => "code_test",
        _ => unreachable!("validated kind"),
    };
    let mut args = json!({"path":path,"timeout_secs":request.timeout_secs.unwrap_or(60)});
    if let Some(filter) = request.filter {
        args["filter"] = json!(filter);
    }
    Ok((tool, args))
}
