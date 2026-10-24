/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::across_projects::{names_word, read_name_scan_text};
use super::declared_at::declared_at;
use super::matching::owner_path_matches;
use super::types::SymbolHit;
use std::path::Path;

pub(crate) fn find_source_members(
    root: &Path,
    path: &Path,
    owner: &[&str],
    member: &str,
) -> Vec<SymbolHit> {
    // A Rust source fallback must prove both the declaration kind and its enclosing type.
    // A `mod owner;` line never makes locals or free functions members of that owner.
    if path.extension().is_some_and(|ext| ext == "rs") {
        let declarations = super::search::local_declarations_in_file(path, member);
        // The lightweight source scanner cannot distinguish fields from parameters. If
        // either is present, defer to the index rather than silently dropping a field
        // that shares its name with a method (#1128).
        if declarations
            .iter()
            .any(|hit| matches!(hit.kind, "Field" | "Property" | "Variable"))
        {
            return Vec::new();
        }
        return declarations
            .into_iter()
            .filter(|hit| {
                matches!(
                    hit.kind,
                    "Function" | "Method" | "Constant" | "TypeParameter"
                )
            })
            .filter(|hit| hit.container.is_some())
            .filter(|hit| super::matching::qualifier_matches(root, hit, owner))
            .collect();
    }
    let Some(text) = read_name_scan_text(path) else {
        return Vec::new();
    };
    let type_name = owner.last().copied().unwrap_or_default();
    if !names_word(&text, member) || (!type_name.is_empty() && !names_word(&text, type_name)) {
        return Vec::new();
    }
    if !type_name.is_empty() && !owner_path_matches(root, path, owner, &[type_name.to_string()]) {
        return Vec::new();
    }
    let is_go = path.extension().and_then(|e| e.to_str()) == Some("go");
    let file_declares_type =
        type_name.is_empty() || text.lines().any(|l| declared_at(l, type_name).is_some());
    let mut hits = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let Some(col) = declared_at(line, member) else {
            continue;
        };
        let trimmed = line.trim_start();
        if is_go {
            if trimmed.starts_with("func") {
                let prefix: String = line.chars().take(col).collect();
                if !type_name.is_empty() && !names_word(&prefix, type_name) {
                    continue;
                }
            } else {
                // Go composite literal keys and labels are not member declarations (#1110).
                let after: String = line.chars().skip(col + member.chars().count()).collect();
                if !file_declares_type || after.trim_start().starts_with(':') {
                    continue;
                }
            }
        } else if ["struct ", "enum ", "trait ", "class ", "interface ", "mod "]
            .iter()
            .any(|kw| trimmed.starts_with(kw) || trimmed.contains(&format!(" {kw}")))
            || (!file_declares_type && !trimmed.contains("fn "))
        {
            continue;
        }
        hits.push(SymbolHit {
            path: path.to_path_buf(),
            name: member.to_string(),
            kind: if trimmed.starts_with("func") || trimmed.contains("fn ") {
                "Method"
            } else {
                "Property"
            },
            container: Some(owner.join("::")),
            line: index as u32 + 1,
            col: col as u32 + 1,
        });
    }
    hits
}
