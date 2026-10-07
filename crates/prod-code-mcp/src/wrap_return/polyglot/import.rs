/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use crate::parameter_object::Language;

/// Proves whether `caller_path` genuinely imports or shares scope with `decl_file` for `fn_name`.
/// Used when semantic references are empty to prevent cross-file text fallback from rewriting
/// unrelated local aliases or same-named symbols in other files (#982).
#[cfg(test)]
pub(crate) fn proves_cross_file_import(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
    lang: Language,
) -> bool {
    !imported_caller_symbols(content, caller_path, decl_file, fn_name, lang).is_empty()
}

/// Finds the local symbol name(s) under which `decl_file::fn_name` is imported or accessible in `caller_path`.
/// Returns an empty list if `caller_path` does not import or share scope with `decl_file` for `fn_name`.
pub(crate) fn imported_caller_symbols(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
    lang: Language,
) -> Vec<String> {
    let decl_stem = decl_file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if decl_stem.is_empty() {
        return Vec::new();
    }
    let decl_pkg_stem = if decl_stem == "__init__" || decl_stem == "index" {
        decl_file
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or(decl_stem)
    } else {
        decl_stem
    };

    match lang {
        Language::TypeScript | Language::JavaScript => {
            let mut syms = ts_js_imported_symbols(content, decl_stem, fn_name);
            if syms.is_empty() && decl_pkg_stem != decl_stem {
                syms = ts_js_imported_symbols(content, decl_pkg_stem, fn_name);
            }
            syms
        }
        Language::Python => {
            let mut syms = python_imported_symbols(content, decl_stem, fn_name);
            if syms.is_empty() && decl_pkg_stem != decl_stem {
                syms = python_imported_symbols(content, decl_pkg_stem, fn_name);
            }
            if syms.is_empty()
                && decl_stem == "__init__"
                && caller_path.parent() == decl_file.parent()
            {
                syms = python_imported_symbols(content, "", fn_name);
            }
            syms
        }
        Language::Go | Language::Swift if caller_path.parent() == decl_file.parent() => {
            vec![fn_name.to_string()]
        }
        Language::Cpp | Language::C
            if c_cpp_proves_import(content, caller_path, decl_file, fn_name) =>
        {
            vec![fn_name.to_string()]
        }
        _ => Vec::new(),
    }
}

#[path = "import_cpp.rs"]
mod import_cpp;
use import_cpp::c_cpp_proves_import;

fn extract_specifier(s: &str) -> &str {
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            let start = i + 1;
            for (j, end_c) in chars {
                if end_c == quote {
                    return &s[start..j];
                }
            }
            return &s[start..];
        } else if c == '<' {
            let start = i + 1;
            for (j, end_c) in chars {
                if end_c == '>' {
                    return &s[start..j];
                }
            }
            return &s[start..];
        }
    }
    ""
}

fn specifier_matches_stem(specifier: &str, decl_stem: &str) -> bool {
    if specifier.is_empty() {
        return false;
    }
    let trimmed = specifier.trim();
    let mod_file = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let mod_stem = mod_file
        .strip_suffix(".ts")
        .or_else(|| mod_file.strip_suffix(".tsx"))
        .or_else(|| mod_file.strip_suffix(".js"))
        .or_else(|| mod_file.strip_suffix(".jsx"))
        .or_else(|| mod_file.strip_suffix(".mjs"))
        .or_else(|| mod_file.strip_suffix(".cjs"))
        .unwrap_or(mod_file);
    mod_stem == decl_stem
}

fn clause_imported_symbols(clause: &str, fn_name: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    let trimmed = clause.trim();
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
            return symbols;
        }
    }
    // Default import or bare name: import fn_name from "..."
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.contains(&fn_name) {
        symbols.push(fn_name.to_string());
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
                let before_ok = i == 0
                    || s[..i]
                        .chars()
                        .next_back()
                        .map_or(true, |p| !p.is_alphanumeric() && p != '_' && p != '$');
                let after = &s[i + 4..];
                let after_ok = after
                    .chars()
                    .next()
                    .map_or(true, |n| !n.is_alphanumeric() && n != '_' && n != '$');
                if before_ok && after_ok {
                    return Some((&s[..i], after));
                }
            }
            _ => {}
        }
    }
    None
}

fn is_ident_boundary(c: char) -> bool {
    !c.is_alphanumeric() && c != '_' && c != '$'
}

fn find_import_keyword_starts(content: &str) -> Vec<usize> {
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
        if before_ok && after_ok {
            starts.push(abs_pos);
        }
        search_idx = after_idx;
    }
    starts
}

fn ts_js_imported_symbols(content: &str, decl_stem: &str, fn_name: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    let starts = find_import_keyword_starts(content);
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(content.len());
        let part = &content[start + "import".len()..end];
        if let Some((clause, rest)) = split_import_from(part) {
            let specifier = extract_specifier(rest);
            if specifier_matches_stem(specifier, decl_stem) {
                symbols.extend(clause_imported_symbols(clause, fn_name));
            }
        }
    }
    let mut search_idx = 0;
    while let Some(req_pos) = content[search_idx..].find("require(") {
        let abs_req_pos = search_idx + req_pos;
        let rest = &content[abs_req_pos + "require(".len()..];
        let specifier = extract_specifier(rest);
        if specifier_matches_stem(specifier, decl_stem) {
            let line_before = content[..abs_req_pos]
                .lines()
                .next_back()
                .unwrap_or("")
                .trim();
            if let Some((lhs, _)) = line_before.split_once('=') {
                let lhs = lhs.trim();
                if let (Some(open), Some(close)) = (lhs.find('{'), lhs.rfind('}')) {
                    if open < close {
                        let inner = &lhs[open + 1..close];
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
                    let lhs_clean = lhs
                        .trim_start_matches("const")
                        .trim_start_matches("let")
                        .trim_start_matches("var")
                        .trim_start_matches("import")
                        .trim();
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
        search_idx = abs_req_pos + "require(".len();
    }
    symbols
}

fn python_imported_symbols(content: &str, decl_stem: &str, fn_name: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        let code_line = line.split('#').next().unwrap_or("").trim();
        if code_line.is_empty() {
            continue;
        }
        let mut full_stmt = code_line.to_string();
        if full_stmt.contains('(') && !full_stmt.contains(')') {
            while let Some(next_line) = lines.next() {
                let next_code = next_line.split('#').next().unwrap_or("").trim();
                if !next_code.is_empty() {
                    full_stmt.push(' ');
                    full_stmt.push_str(next_code);
                }
                if next_code.contains(')') {
                    break;
                }
            }
        } else if full_stmt.ends_with('\\') {
            while full_stmt.ends_with('\\') {
                full_stmt.pop();
                if let Some(next_line) = lines.next() {
                    let next_code = next_line.split('#').next().unwrap_or("").trim();
                    if !next_code.is_empty() {
                        full_stmt.push(' ');
                        full_stmt.push_str(next_code);
                    }
                } else {
                    break;
                }
            }
        }

        let trimmed = full_stmt.trim();
        if let Some(rest) = trimmed.strip_prefix("from ") {
            if let Some((mod_part, clause)) = rest.split_once(" import ") {
                let mod_name = mod_part
                    .trim()
                    .rsplit('.')
                    .next()
                    .unwrap_or(mod_part.trim());
                let mod_stem = mod_name.trim_start_matches('.');
                let clause = clause
                    .trim()
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .trim();

                if mod_stem == decl_stem {
                    if clause == "*" {
                        symbols.push(fn_name.to_string());
                    } else {
                        for item in clause.split(',') {
                            let parts: Vec<&str> = item.split_whitespace().collect();
                            match parts.as_slice() {
                                [name] if *name == fn_name => symbols.push(fn_name.to_string()),
                                [orig, "as", local] if *orig == fn_name => {
                                    symbols.push((*local).to_string());
                                }
                                _ => {}
                            }
                        }
                    }
                } else {
                    for item in clause.split(',') {
                        let parts: Vec<&str> = item.split_whitespace().collect();
                        match parts.as_slice() {
                            [name] if *name == decl_stem => symbols.push(fn_name.to_string()),
                            [orig, "as", _local] if *orig == decl_stem => {
                                symbols.push(fn_name.to_string());
                            }
                            _ => {}
                        }
                    }
                }
            }
        } else if let Some(rest) = trimmed.strip_prefix("import ") {
            let rest = rest
                .trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .trim();
            for entry in rest.split(',') {
                let (mod_part, _alias) = entry.split_once(" as ").unwrap_or((entry, ""));
                let mod_name = mod_part
                    .trim()
                    .rsplit('.')
                    .next()
                    .unwrap_or(mod_part.trim());
                if mod_name == decl_stem {
                    symbols.push(fn_name.to_string());
                }
            }
        }
    }
    symbols
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
