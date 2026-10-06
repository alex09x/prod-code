/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::env;
use std::path::{Path, PathBuf};

/// Dynamically detect the base repository name if current directory is a git worktree or repository.
pub fn detect_workspace_name(dir: &Path) -> Option<String> {
    // Every git worktree gets its own isolated server workspace; see
    // `prod_code_mcp::sync::workspace_identity`.
    Some(prod_code_mcp::sync::workspace_identity(dir).name)
}

/// Find the root of the workspace or git worktree containing the specified file.
pub fn find_workspace_root(file_path: &Path) -> Option<PathBuf> {
    let abs_path = if file_path.is_absolute() {
        std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf())
    } else if let Ok(cwd) = env::current_dir() {
        let joined = cwd.join(file_path);
        std::fs::canonicalize(&joined).unwrap_or(joined)
    } else {
        file_path.to_path_buf()
    };

    let mut current = if abs_path.is_file() {
        abs_path.parent()?
    } else {
        abs_path.as_path()
    };

    let mut candidate_manifest = None;

    loop {
        if current.join(".git").exists() {
            return Some(current.to_path_buf());
        }
        if candidate_manifest.is_none()
            && (current.join("Cargo.toml").exists()
                || current.join("go.mod").exists()
                || current.join("package.json").exists()
                || current.join("pyproject.toml").exists()
                || current.join("Package.swift").exists())
        {
            candidate_manifest = Some(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }

    candidate_manifest
}

/// Scans a directory to locate the first recognizable source code file and declaration offset.
pub fn find_first_code_file(dir: &Path) -> Option<(PathBuf, u32, u32)> {
    let mut builder = ignore::WalkBuilder::new(dir);
    builder.hidden(true).git_ignore(true).max_depth(Some(4));

    for entry in builder.build().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "rs" | "go" | "py" | "ts") || path.to_string_lossy().contains("/tests/") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };

        for (line_idx, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//")
                || trimmed.starts_with("/*")
                || trimmed.starts_with("#")
                || trimmed.is_empty()
            {
                continue;
            }
            if let Some(pos) = trimmed.find("pub const ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 12) as u32));
            }
            if let Some(pos) = trimmed.find("pub struct ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 13) as u32));
            }
            if let Some(pos) = trimmed.find("pub fn ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 9) as u32));
            }
            if let Some(pos) = trimmed.find("func ") {
                return Some((path.to_path_buf(), line_idx as u32, (pos + 6) as u32));
            }
        }
    }
    None
}
