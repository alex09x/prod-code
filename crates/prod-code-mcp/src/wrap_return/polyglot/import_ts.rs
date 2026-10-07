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
use crate::caller_migration::mask::lexical_code_mask;
use crate::parameter_object::Language;

fn strip_ext(p: &Path) -> PathBuf {
    if let Some(stem) = p.file_stem() {
        p.with_file_name(stem)
    } else {
        p.to_path_buf()
    }
}

fn resolve_relative_path(base_dir: &Path, rel_spec: &str) -> PathBuf {
    let mut path = base_dir.to_path_buf();
    for component in rel_spec.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            c => {
                path.push(c);
            }
        }
    }
    path
}

pub(crate) fn specifier_matches_decl(
    specifier: &str,
    caller_path: &Path,
    decl_file: &Path,
) -> bool {
    if specifier.is_empty() {
        return false;
    }
    let trimmed = specifier.trim();
    let decl_stem = decl_file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let decl_pkg_stem = if decl_stem == "index" || decl_stem == "__init__" {
        decl_file
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or(decl_stem)
    } else {
        decl_stem
    };

    if trimmed.starts_with('.') {
        let caller_dir = caller_path.parent().unwrap_or_else(|| Path::new(""));
        let resolved = resolve_relative_path(caller_dir, trimmed);
        let resolved_clean = strip_ext(&resolved);
        let decl_clean = strip_ext(decl_file);
        if resolved_clean == decl_clean {
            return true;
        }
        if (decl_stem == "index" || decl_stem == "__init__")
            && decl_file.parent() == Some(&resolved)
        {
            return true;
        }
        false
    } else {
        let mod_file = trimmed.rsplit('/').next().unwrap_or(trimmed);
        let mod_stem = mod_file
            .strip_suffix(".ts")
            .or_else(|| mod_file.strip_suffix(".tsx"))
            .or_else(|| mod_file.strip_suffix(".js"))
            .or_else(|| mod_file.strip_suffix(".jsx"))
            .or_else(|| mod_file.strip_suffix(".mjs"))
            .or_else(|| mod_file.strip_suffix(".cjs"))
            .unwrap_or(mod_file);
        mod_stem == decl_stem || mod_stem == decl_pkg_stem
    }
}

fn clean_lhs_binding(lhs: &str) -> &str {
    let trimmed = lhs.trim();
    for kw in &["const", "let", "var", "import"] {
        if let Some(rest) = trimmed.strip_prefix(kw) {
            if rest.starts_with(char::is_whitespace) {
                return rest.trim();
            }
        }
    }
    trimmed
}

fn clause_imported_symbols(clause: &str, fn_name: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    let mut trimmed = clause.trim();
    if let Some(rest) = trimmed.strip_prefix("type") {
        if rest.starts_with(char::is_whitespace) {
            trimmed = rest.trim();
        }
    }
    if trimmed.contains('*') {
        symbols.push(fn_name.to_string());
        return symbols;
    }
    if let (Some(open), Some(close)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if open < close {
            let inner = &trimmed[open + 1..close];
            for item in inner.split(',') {
                let parts: Vec<&str> = item.split_whitespace().collect();
                match parts.as_slice() {
                    [name] if *name == fn_name => symbols.push(fn_name.to_string()),
                    [orig, "as", local] if *orig == fn_name => symbols.push((*local).to_string()),
                    _ => {}
                }
            }
        }
        let default_part = trimmed[..open].trim().trim_end_matches(',').trim();
        if !default_part.is_empty()
            && default_part
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        {
            symbols.push(default_part.to_string());
        }
        return symbols;
    }

    let default_ident = trimmed
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect::<String>();
    if !default_ident.is_empty() {
        symbols.push(default_ident);
    }
    symbols
}

fn split_import_from(s: &str) -> Option<(&str, &str)> {
    let (mut b_depth, mut p_depth) = (0usize, 0usize);
    for (i, c) in s.char_indices() {
        match c {
            '{' => b_depth += 1,
            '}' => b_depth = b_depth.saturating_sub(1),
            '(' => p_depth += 1,
            ')' => p_depth = p_depth.saturating_sub(1),
            'f' if b_depth == 0 && p_depth == 0 && s[i..].starts_with("from") => {
                let before_ok =
                    i == 0 || s[..i].chars().next_back().map_or(true, is_ident_boundary);
                let after = &s[i + 4..];
                let after_ok = after.chars().next().map_or(true, is_ident_boundary);
                if before_ok && after_ok {
                    return Some((&s[..i], after));
                }
            }
            ';' if b_depth == 0 && p_depth == 0 => {
                return None;
            }
            _ => {}
        }
    }
    None
}

fn is_ident_boundary(c: char) -> bool {
    !c.is_alphanumeric() && c != '_' && c != '$'
}

fn find_import_keyword_starts(content: &str, mask: &[bool]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut search_idx = 0;
    while let Some(pos) = content[search_idx..].find("import") {
        let abs_pos = search_idx + pos;
        let before_ok = if abs_pos == 0 {
            true
        } else {
            content[..abs_pos]
                .chars()
                .next_back()
                .map_or(true, is_ident_boundary)
        };
        let after_idx = abs_pos + "import".len();
        let after_ok = content[after_idx..]
            .chars()
            .next()
            .map_or(true, is_ident_boundary);
        if before_ok && after_ok && abs_pos < mask.len() && mask[abs_pos] {
            starts.push(abs_pos);
        }
        search_idx = after_idx;
    }
    starts
}

pub(crate) fn ts_js_imported_symbols(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
) -> Vec<String> {
    let mut symbols = Vec::new();
    let mask = lexical_code_mask(content, Language::TypeScript);
    let starts = find_import_keyword_starts(content, &mask);
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(content.len());
        let part = &content[start + "import".len()..end];
        if let Some((clause, rest)) = split_import_from(part) {
            let specifier = extract_specifier(rest);
            if specifier_matches_decl(specifier, caller_path, decl_file) {
                symbols.extend(clause_imported_symbols(clause, fn_name));
            }
        }
    }
    let mut search_idx = 0;
    while let Some(req_offset) = content[search_idx..].find("require") {
        let abs_req_pos = search_idx + req_offset;
        let before_ok = abs_req_pos == 0 || {
            let prev = content[..abs_req_pos].chars().next_back().unwrap();
            is_ident_boundary(prev)
        };
        let after = &content[abs_req_pos + "require".len()..];
        let not_ident = after.chars().next().map_or(true, is_ident_boundary);
        let trimmed = after.trim_start();
        if before_ok
            && not_ident
            && trimmed.starts_with('(')
            && abs_req_pos < mask.len()
            && mask[abs_req_pos]
        {
            let paren_open = abs_req_pos + "require".len() + (after.len() - trimmed.len());
            let mut p_depth = 0usize;
            let mut close_paren = None;
            for (offset, c) in content[paren_open..].char_indices() {
                match c {
                    '(' => p_depth += 1,
                    ')' => {
                        p_depth -= 1;
                        if p_depth == 0 {
                            close_paren = Some(paren_open + offset);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(close) = close_paren {
                let inside = &content[paren_open + 1..close];
                let specifier = extract_specifier(inside);
                if specifier_matches_decl(specifier, caller_path, decl_file) {
                    let decl_before = content[..abs_req_pos]
                        .rsplit(';')
                        .next()
                        .unwrap_or("")
                        .trim();
                    let after_close = content[close + 1..].trim_start();
                    if let Some(prop_rest) = after_close.strip_prefix('.') {
                        let prop_rest = prop_rest.trim_start();
                        let prop_name: String = prop_rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                            .collect();
                        if prop_name == fn_name {
                            if let Some((lhs, _)) = decl_before.rsplit_once('=') {
                                let lhs_clean = clean_lhs_binding(lhs);
                                if !lhs_clean.is_empty()
                                    && lhs_clean
                                        .chars()
                                        .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
                                {
                                    symbols.push(lhs_clean.to_string());
                                }
                            }
                        }
                    } else if let Some((lhs, _)) = decl_before.rsplit_once('=') {
                        let lhs = lhs.trim();
                        if let (Some(open), Some(close_brace)) = (lhs.find('{'), lhs.rfind('}')) {
                            if open < close_brace {
                                let inner = &lhs[open + 1..close_brace];
                                for item in inner.split(',') {
                                    let item = item.trim();
                                    if let Some((orig, local)) = item.split_once(':') {
                                        if orig.trim() == fn_name {
                                            symbols.push(local.trim().to_string());
                                        }
                                    } else if item == fn_name {
                                        symbols.push(fn_name.to_string());
                                    }
                                }
                            }
                        } else {
                            let lhs_clean = clean_lhs_binding(lhs);
                            if !lhs_clean.is_empty()
                                && lhs_clean
                                    .chars()
                                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
                            {
                                symbols.push(fn_name.to_string());
                            }
                        }
                    }
                }
                search_idx = close + 1;
                continue;
            }
        }
        search_idx = abs_req_pos + "require".len();
    }
    symbols
}

#[cfg(test)]
#[path = "import_ts_tests.rs"]
mod tests;
