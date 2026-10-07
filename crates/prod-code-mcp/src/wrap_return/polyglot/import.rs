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

#[path = "import_python.rs"]
mod import_python;
use import_python::python_imported_symbols;

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
    while let Some(req_offset) = content[search_idx..].find("require") {
        let abs_req_pos = search_idx + req_offset;
        let before_ok = abs_req_pos == 0 || {
            let prev = content[..abs_req_pos].chars().next_back().unwrap();
            !prev.is_alphanumeric() && prev != '_' && prev != '$'
        };
        let after = &content[abs_req_pos + "require".len()..];
        let not_ident = after
            .chars()
            .next()
            .map_or(true, |c| !c.is_alphanumeric() && c != '_' && c != '$');
        let trimmed = after.trim_start();
        if before_ok && not_ident && trimmed.starts_with('(') {
            let paren_open = abs_req_pos + "require".len() + (after.len() - trimmed.len());
            let rest = &content[paren_open + 1..];
            let specifier = extract_specifier(rest);
            if specifier_matches_stem(specifier, decl_stem) {
                let decl_before = content[..abs_req_pos]
                    .rsplit(';')
                    .next()
                    .unwrap_or("")
                    .trim();
                if let Some((lhs, _)) = decl_before.rsplit_once('=') {
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
            search_idx = paren_open + 1;
        } else {
            search_idx = abs_req_pos + "require".len();
        }
    }
    symbols
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
