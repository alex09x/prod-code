/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use super::extract_specifier;
use super::resolve_relative_path;
use super::specifier_matches_decl_depth;

pub(crate) fn resolve_reexport_file(caller_dir: &Path, rel_spec: &str) -> Option<PathBuf> {
    let resolved = resolve_relative_path(caller_dir, rel_spec);
    if resolved.is_file() {
        return Some(resolved);
    }
    for ext in &["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
        let with_ext = resolved.with_extension(ext);
        if with_ext.is_file() {
            return Some(with_ext);
        }
    }
    for index_name in &["index.ts", "index.tsx", "index.js", "index.jsx"] {
        let index_file = resolved.join(index_name);
        if index_file.is_file() {
            return Some(index_file);
        }
    }
    None
}

pub(crate) fn file_reexports_decl(
    content: &str,
    current_file: &Path,
    decl_file: &Path,
    depth: usize,
) -> bool {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("export") && trimmed.contains("from") {
            let spec = extract_specifier(trimmed);
            if !spec.is_empty()
                && specifier_matches_decl_depth(spec, current_file, decl_file, depth)
            {
                return true;
            }
        }
    }
    false
}
