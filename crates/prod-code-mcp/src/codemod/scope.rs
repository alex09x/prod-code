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
use std::path::{Path, PathBuf};

use super::types::{CODE_EXTENSIONS, SKIPPED_DIRS};

/// Recursively collect all code files in `dir`.
pub fn collect_code_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            continue;
        }

        if path.is_dir() {
            if !SKIPPED_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                collect_code_files(&path, out);
            }
        } else if path.is_file()
            && let Some(ext) = path.extension().and_then(|e| e.to_str())
            && CODE_EXTENSIONS.contains(&ext)
        {
            out.push(path);
        }
    }
}

/// Resolve a user supplied relative codemod scope without traversing links or leaving the
/// workspace. The path must already exist as a file or directory.
pub fn resolve_workspace_scope(workspace_root: &Path, raw: &str) -> Result<PathBuf> {
    let raw_trimmed = raw.trim();
    anyhow::ensure!(
        !raw_trimmed.is_empty(),
        "codemod path must name a file or directory"
    );
    let requested = Path::new(raw);
    anyhow::ensure!(
        !requested.is_absolute(),
        "codemod path must be relative to the workspace"
    );
    let mut has_curdir = false;
    let mut components = Vec::new();
    for component in requested.components() {
        match component {
            std::path::Component::CurDir => {
                has_curdir = true;
            }
            std::path::Component::Normal(name) => components.push(name),
            std::path::Component::ParentDir => {
                anyhow::bail!("codemod path cannot contain parent traversal")
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                anyhow::bail!("codemod path must be relative to the workspace")
            }
        }
    }
    let is_root_scope = components.is_empty() && has_curdir;
    anyhow::ensure!(
        !components.is_empty() || is_root_scope,
        "codemod path must name a file or directory"
    );
    let root = workspace_root
        .canonicalize()
        .context("cannot resolve workspace root")?;
    let mut target = root.clone();
    for component in components {
        target.push(component);
        let metadata = std::fs::symlink_metadata(&target).with_context(|| {
            format!(
                "codemod path component does not exist: {}",
                target.display()
            )
        })?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "codemod path cannot traverse symlink {}",
            target.display()
        );
    }
    let metadata = std::fs::metadata(&target)?;
    anyhow::ensure!(
        metadata.is_file() || metadata.is_dir(),
        "codemod path must be a file or directory"
    );
    anyhow::ensure!(target.starts_with(&root), "codemod path escapes workspace");
    Ok(target)
}

pub fn validate_scope_path(workspace_root: &Path, scope: &Path) -> Result<PathBuf> {
    let root = workspace_root
        .canonicalize()
        .context("cannot resolve workspace root")?;
    let relative = scope
        .strip_prefix(&root)
        .context("codemod scope is outside the workspace")?;
    let mut target = root.clone();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(name) => target.push(name),
            _ => anyhow::bail!("codemod scope contains an unsafe path component"),
        }
        let metadata = std::fs::symlink_metadata(&target)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "codemod scope traverses a symlink"
        );
    }
    anyhow::ensure!(
        target.is_file() || target.is_dir(),
        "codemod scope does not exist"
    );
    Ok(target)
}
