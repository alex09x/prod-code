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

use super::direct_specifier_matches_decl;
use super::extract_specifier;
use super::resolve_relative_path;

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

pub(crate) fn barrel_exports_symbol_from_decl(
    barrel_file: &Path,
    exported_symbol: &str,
    decl_file: &Path,
    decl_fn: &str,
    depth: usize,
) -> bool {
    if depth > 5 {
        return false;
    }
    let Ok(content) = std::fs::read_to_string(barrel_file) else {
        return false;
    };
    let barrel_dir = barrel_file.parent().unwrap_or_else(|| Path::new(""));

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("export") || !trimmed.contains("from") {
            continue;
        }
        let specifier = extract_specifier(trimmed);
        if specifier.is_empty() {
            continue;
        }
        let before_from = match trimmed.split_once("from") {
            Some((before, _)) => before.trim(),
            None => continue,
        };
        let clause = match before_from.strip_prefix("export") {
            Some(c) => c.trim(),
            None => continue,
        };
        let clause = if let Some(rest) = clause.strip_prefix("type") {
            if rest.starts_with(char::is_whitespace) {
                rest.trim()
            } else {
                clause
            }
        } else {
            clause
        };

        let mut source_symbols = Vec::new();
        if clause.starts_with('*') {
            if !clause.contains(" as ") {
                source_symbols.push(exported_symbol.to_string());
            }
        } else if let (Some(open), Some(close)) = (clause.find('{'), clause.rfind('}')) {
            if open < close {
                let inner = &clause[open + 1..close];
                for item in inner.split(',') {
                    let parts: Vec<&str> = item.split_whitespace().collect();
                    match parts.as_slice() {
                        [name] if *name == exported_symbol => {
                            source_symbols.push(exported_symbol.to_string());
                        }
                        [orig, "as", local] if *local == exported_symbol => {
                            source_symbols.push((*orig).to_string());
                        }
                        _ => {}
                    }
                }
            }
        }

        for source_symbol in source_symbols {
            if direct_specifier_matches_decl(specifier, barrel_file, decl_file) {
                if source_symbol == decl_fn {
                    return true;
                }
            } else if let Some(sub_barrel) = resolve_reexport_file(barrel_dir, specifier) {
                if barrel_exports_symbol_from_decl(
                    &sub_barrel,
                    &source_symbol,
                    decl_file,
                    decl_fn,
                    depth + 1,
                ) {
                    return true;
                }
            }
        }
    }
    false
}
