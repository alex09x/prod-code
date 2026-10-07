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
pub(crate) fn proves_cross_file_import(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
    lang: Language,
) -> bool {
    let decl_stem = decl_file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if decl_stem.is_empty() {
        return false;
    }

    match lang {
        Language::TypeScript | Language::JavaScript => {
            ts_js_proves_import(content, decl_stem, fn_name)
        }
        Language::Python => python_proves_import(content, decl_stem, fn_name),
        Language::Go => {
            // In Go, files in the same directory share package scope.
            caller_path.parent() == decl_file.parent()
        }
        Language::Cpp | Language::C => {
            c_cpp_proves_import(content, caller_path, decl_file, fn_name)
        }
        Language::Swift => caller_path.parent() == decl_file.parent(),
        _ => false,
    }
}

fn c_cpp_proves_import(content: &str, caller_path: &Path, decl_file: &Path, fn_name: &str) -> bool {
    let decl_stem = decl_file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let decl_name = decl_file.file_name().and_then(|s| s.to_str()).unwrap_or("");

    if caller_path.file_stem().and_then(|s| s.to_str()) == Some(decl_stem) {
        return true;
    }

    if content.lines().any(|l| {
        let trimmed = l.trim();
        trimmed.starts_with("#include")
            && (trimmed.contains(decl_name) || trimmed.contains(decl_stem))
    }) {
        return true;
    }

    if let Ok(decl_content) = std::fs::read_to_string(decl_file) {
        let caller_name = caller_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let caller_stem = caller_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        let mut decl_headers = Vec::new();
        for line in decl_content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("#include") {
                let spec = extract_specifier(trimmed);
                if !spec.is_empty() {
                    decl_headers.push(spec);
                }
            }
        }

        for h in &decl_headers {
            let h_file = h.rsplit('/').next().unwrap_or(h);
            let h_stem = h_file
                .strip_suffix(".h")
                .or_else(|| h_file.strip_suffix(".hpp"))
                .or_else(|| h_file.strip_suffix(".hxx"))
                .unwrap_or(h_file);
            if (caller_name == h_file || caller_stem == h_stem) && content.contains(fn_name) {
                return true;
            }
        }

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("#include") {
                let spec = extract_specifier(trimmed);
                for h in &decl_headers {
                    let h_file = h.rsplit('/').next().unwrap_or(h);
                    let h_stem = h_file
                        .strip_suffix(".h")
                        .or_else(|| h_file.strip_suffix(".hpp"))
                        .or_else(|| h_file.strip_suffix(".hxx"))
                        .unwrap_or(h_file);
                    if spec == *h || spec.contains(h_file) || spec.contains(h_stem) {
                        return true;
                    }
                }
            }
        }
    }

    false
}

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

fn clause_imports_name(clause: &str, fn_name: &str) -> bool {
    let trimmed = clause.trim();
    if trimmed.contains('*') {
        return true;
    }
    if let (Some(open), Some(close)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if open < close {
            let inner = &trimmed[open + 1..close];
            for item in inner.split(',') {
                let parts: Vec<&str> = item.split_whitespace().collect();
                match parts.as_slice() {
                    [name] if *name == fn_name => return true,
                    [_orig, "as", local] if *local == fn_name => return true,
                    [orig, "as", _local] if *orig == fn_name => {
                        // Aliased away to a different name, so calls to fn_name
                        // do not refer to this imported symbol.
                        return false;
                    }
                    _ => {}
                }
            }
            return false;
        }
    }
    // Default import or bare name: import fn_name from "..."
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    words.contains(&fn_name)
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

fn ts_js_proves_import(content: &str, decl_stem: &str, fn_name: &str) -> bool {
    for part in content.split("import") {
        if let Some((clause, rest)) = split_import_from(part) {
            let specifier = extract_specifier(rest);
            if specifier_matches_stem(specifier, decl_stem) && clause_imports_name(clause, fn_name)
            {
                return true;
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
                                let orig = orig.trim();
                                let local = local.trim();
                                if orig == fn_name && local != fn_name {
                                    return false;
                                }
                                if local == fn_name {
                                    return true;
                                }
                            } else if item == fn_name {
                                return true;
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
                        return true;
                    }
                }
            }
        }
        search_idx = abs_req_pos + "require(".len();
    }
    false
}

fn python_proves_import(content: &str, decl_stem: &str, fn_name: &str) -> bool {
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
                if mod_stem == decl_stem {
                    let clause = clause
                        .trim()
                        .trim_start_matches('(')
                        .trim_end_matches(')')
                        .trim();
                    if clause == "*" {
                        return true;
                    }
                    for item in clause.split(',') {
                        let parts: Vec<&str> = item.split_whitespace().collect();
                        match parts.as_slice() {
                            [name] if *name == fn_name => return true,
                            [_orig, "as", local] if *local == fn_name => return true,
                            [orig, "as", _local] if *orig == fn_name => {
                                return false;
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
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
