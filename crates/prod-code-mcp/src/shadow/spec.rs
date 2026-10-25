/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{HypothesisEdit, HypothesisSpec};
use anyhow::{Context, Result};
use std::path::{Component, Path, PathBuf};

/// The `/`-separated path of `file` inside `root`.
pub fn relative_edit_path(root: &Path, file: &Path) -> Result<String> {
    let root = std::fs::canonicalize(root).context("cannot resolve workspace root")?;
    let file = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    anyhow::ensure!(
        !file.components().any(|c| matches!(c, Component::ParentDir)),
        "{} is outside the workspace",
        file.display()
    );
    // Resolve the nearest existing ancestor, leaving the leaf alone for deletion.
    let parent = file.parent().context("an edit needs a parent directory")?;
    let name = file.file_name().context("an edit needs a file name")?;
    let file = resolve_parent(parent)?.join(name);
    let rel = file
        .strip_prefix(&root)
        .with_context(|| format!("{} is outside the workspace", file.display()))?;
    let rel = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    anyhow::ensure!(!rel.is_empty(), "an edit needs a file path");
    Ok(rel)
}

fn resolve_parent(parent: &Path) -> Result<PathBuf> {
    let mut at = parent.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(&at) {
            Ok(mut real) => {
                anyhow::ensure!(real.is_dir(), "{} is not a directory", real.display());
                real.extend(missing.iter().rev());
                return Ok(real);
            }
            Err(error) => {
                match std::fs::symlink_metadata(&at) {
                    Err(meta)
                        if error.kind() == std::io::ErrorKind::NotFound
                            && meta.kind() == std::io::ErrorKind::NotFound => {}
                    _ => anyhow::bail!("cannot resolve {}: {error}", at.display()),
                }
                missing.push(
                    at.file_name()
                        .context("parent has no existing ancestor")?
                        .to_os_string(),
                );
                at = at
                    .parent()
                    .context("parent has no existing ancestor")?
                    .to_path_buf();
            }
        }
    }
}

/// Parses the `hypotheses` array shared by the MCP tool and the CLI spec file: every entry has
/// a `name`, `edits` of `{path, new_text}` or `{path, file}` (the proposed content read from
/// that local file, relative to `file_base`) and optional `delete` paths. A hypothesis without
/// edits is the baseline.
pub fn parse_specs(
    root: &Path,
    json: &serde_json::Value,
    file_base: Option<&Path>,
) -> Result<Vec<HypothesisSpec>> {
    let list = json
        .get("hypotheses")
        .and_then(|v| v.as_array())
        .context("missing 'hypotheses' array")?;
    anyhow::ensure!(!list.is_empty(), "'hypotheses' is empty");
    let mut specs = Vec::with_capacity(list.len());
    for (i, hypothesis) in list.iter().enumerate() {
        let name = hypothesis
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("h{}", i + 1));
        let mut edits = Vec::new();
        for edit in hypothesis
            .get("edits")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let path = edit
                .get("path")
                .and_then(|v| v.as_str())
                .with_context(|| format!("hypothesis {name}: edit without 'path'"))?;
            let text = if let Some(text) = edit.get("new_text").and_then(|v| v.as_str()) {
                text.to_string()
            } else if let Some(file) = edit.get("file").and_then(|v| v.as_str()) {
                let file = Path::new(file);
                let file = if file.is_absolute() {
                    file.to_path_buf()
                } else {
                    file_base.unwrap_or(root).join(file)
                };
                std::fs::read_to_string(&file)
                    .with_context(|| format!("hypothesis {name}: cannot read {}", file.display()))?
            } else {
                anyhow::bail!("hypothesis {name}: edit for {path} needs 'new_text' or 'file'");
            };
            edits.push(HypothesisEdit {
                relative_path: relative_edit_path(root, Path::new(path))?,
                text: Some(text),
            });
        }
        for path in hypothesis
            .get("delete")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            edits.push(HypothesisEdit {
                relative_path: relative_edit_path(root, Path::new(path))?,
                text: None,
            });
        }
        specs.push(HypothesisSpec { name, edits });
    }
    Ok(specs)
}
