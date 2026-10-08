/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::nested_projects::source_files;
use std::path::Path;

/// Finds the original target type of a type alias or re-export (`as Alias` or `type Alias = Target`)
/// across workspace source files, checking `hint` first (#1025).
pub fn find_type_alias_target(root: &Path, type_name: &str, hint: Option<&Path>) -> Option<String> {
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
        .chain(source_files(root).take(100))
    {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        if !text.contains(type_name) {
            continue;
        }
        if let Some(target) = extract_alias_target(&text, type_name) {
            return Some(target);
        }
    }
    None
}

pub fn extract_alias_target(text: &str, alias: &str) -> Option<String> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.contains(alias) {
            continue;
        }
        // Match `AgentTracker as IdentityState`
        if let Some(as_pos) = trimmed.find(&format!("as {alias}")) {
            let before = trimmed[..as_pos].trim_end();
            let ident_start = before.rfind(|c: char| !is_ident(c)).map_or(0, |i| i + 1);
            let target = &before[ident_start..];
            if !target.is_empty() && target != alias {
                return Some(target.to_string());
            }
        }
        // Match `type IdentityState = AgentTracker;`
        if (trimmed.starts_with("type ") || trimmed.starts_with("pub type "))
            && trimmed.contains(&format!("type {alias}"))
            && let Some((_, rhs)) = trimmed.split_once('=')
        {
            let rhs = rhs
                .trim()
                .split([';', '<', ' ', '\t'])
                .next()
                .unwrap_or("")
                .trim();
            let target = rhs.rsplit("::").next().unwrap_or(rhs).trim();
            if !target.is_empty() && target != alias {
                return Some(target.to_string());
            }
        }
    }
    None
}
