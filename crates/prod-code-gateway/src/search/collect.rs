/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::parser::language_of;
use std::path::{Path, PathBuf};

pub(crate) fn canonical_root(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

/// Normalizes an invalidation path using wire-path rules. Unlike a search scope, it must name a
/// file, so absolute and parent-traversal forms are ignored rather than joined onto `root`.
pub(crate) fn normalize_relative_path(raw: &str) -> Option<String> {
    if raw.starts_with(['/', '\\']) {
        return None;
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return None;
    }
    let mut components = Vec::new();
    for component in raw.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => return None,
            component => components.push(component),
        }
    }
    (!components.is_empty()).then(|| components.join("/"))
}

/// A source path is safe only when every descendant component exists without being a link.
/// `root` itself is already canonicalized, allowing a checkout alias while rejecting arbitrary
/// links below it (including a linked parent directory during incremental invalidation).
pub(crate) fn source_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = normalize_relative_path(rel)?;
    let mut path = root.to_path_buf();
    for component in rel.split('/') {
        path.push(component);
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if meta.file_type().is_symlink() {
            return None;
        }
    }
    Some(path)
}

/// Source files worth indexing, relative path first. Mirrors the sync layer's exclusions.
pub(crate) fn collect_source_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // `Path::is_dir` follows symlinks, which would escape the checkout or recurse forever.
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(
            name.as_str(),
            ".git"
                | "target"
                | "node_modules"
                | ".venv"
                | "venv"
                | "__pycache__"
                | ".pytest_cache"
                | ".mypy_cache"
                | ".build"
                | "dist"
                | "vendor"
        ) || name.starts_with('.') && name != ".config"
        {
            continue;
        }
        if file_type.is_dir() {
            collect_source_files(root, &path, out);
        } else if file_type.is_file()
            && language_of(&name).is_some()
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push((rel.to_string_lossy().replace('\\', "/"), path));
        }
    }
}
