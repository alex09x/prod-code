/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A standard-library name found where the checkout uses it.

use super::sources::find_identifier_on_line;
use super::types::SymbolHit;
use std::path::Path;

/// Resolves standard library symbols (e.g. `std::process::Command`) or workspace usages when
/// not indexed by `workspace/symbol` (#935).
pub(crate) fn find_stdlib_or_usage_hits(
    root: &Path,
    symbol: &str,
    qualifiers: &[&str],
    name: &str,
    hint: Option<&Path>,
) -> Vec<SymbolHit> {
    let mut hits = Vec::new();
    let is_stdlib = matches!(qualifiers.first().copied(), Some("std" | "core" | "alloc"));
    if !is_stdlib {
        return hits;
    }
    let mut files_to_check = Vec::new();
    if let Some(h) = hint {
        let p = if h.is_absolute() {
            h.to_path_buf()
        } else {
            root.join(h)
        };
        if p.is_file() {
            files_to_check.push(p);
        }
    }
    for file in files_to_check
        .into_iter()
        .chain(super::nested_projects::source_files(root).take(200))
    {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        if !text.contains(name) {
            continue;
        }
        for (idx, line) in text.lines().enumerate() {
            if line.contains(symbol)
                || (line.contains(name) && qualifiers.iter().all(|q| line.contains(q)))
            {
                if let Some(col) = find_identifier_on_line(line, name) {
                    hits.push(SymbolHit {
                        path: file.clone(),
                        name: name.to_string(),
                        kind: "Struct",
                        container: Some(qualifiers.join("::")),
                        line: idx as u32 + 1,
                        col: line[..col].chars().count() as u32 + 1,
                    });
                    return hits;
                }
            }
        }
    }
    hits
}
