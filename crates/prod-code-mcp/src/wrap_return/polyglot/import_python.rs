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

fn strip_py_ext(p: &Path) -> PathBuf {
    if p.extension().is_some_and(|e| e == "py" || e == "pyi") {
        p.with_extension("")
    } else {
        p.to_path_buf()
    }
}

fn py_mod_matches_decl(mod_str: &str, caller_path: &Path, decl_file: &Path) -> bool {
    let trimmed = mod_str.trim();
    if trimmed.is_empty() {
        return false;
    }
    let decl_clean = strip_py_ext(decl_file);
    let decl_stem = decl_file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let is_init = decl_stem == "__init__";

    if trimmed.starts_with('.') {
        let num_dots = trimmed.chars().take_while(|c| *c == '.').count();
        let sub_mod = &trimmed[num_dots..];
        let mut base = caller_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf();
        for _ in 1..num_dots {
            base.pop();
        }
        if !sub_mod.is_empty() {
            for part in sub_mod.split('.') {
                if !part.is_empty() {
                    base.push(part);
                }
            }
        }
        if base == decl_clean {
            return true;
        }
        if is_init && decl_clean.parent() == Some(&base) {
            return true;
        }
        false
    } else {
        let mod_path_str = trimmed.replace('.', "/");
        let mod_path = Path::new(&mod_path_str);
        if decl_clean == mod_path || decl_clean.ends_with(mod_path) {
            return true;
        }
        if is_init {
            if let Some(parent) = decl_clean.parent() {
                if parent == mod_path || parent.ends_with(mod_path) {
                    return true;
                }
            }
        }
        if caller_path.parent() == decl_file.parent() && trimmed == decl_stem {
            return true;
        }
        false
    }
}

fn py_mod_matches_pkg(mod_str: &str, caller_path: &Path, decl_file: &Path) -> bool {
    let trimmed = mod_str.trim();
    if trimmed.is_empty() {
        return false;
    }
    let decl_dir = decl_file.parent().unwrap_or_else(|| Path::new(""));

    if trimmed.starts_with('.') {
        let num_dots = trimmed.chars().take_while(|c| *c == '.').count();
        let sub_mod = &trimmed[num_dots..];
        let mut base = caller_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf();
        for _ in 1..num_dots {
            base.pop();
        }
        if !sub_mod.is_empty() {
            for part in sub_mod.split('.') {
                if !part.is_empty() {
                    base.push(part);
                }
            }
        }
        base == decl_dir
    } else {
        let mod_path_str = trimmed.replace('.', "/");
        let mod_path = Path::new(&mod_path_str);
        if decl_dir == mod_path || decl_dir.ends_with(mod_path) {
            return true;
        }
        if caller_path.parent() == decl_file.parent() {
            return true;
        }
        false
    }
}

fn split_semicolon_statements(line: &str) -> Vec<&str> {
    let mut stmts = Vec::new();
    let mut in_quote = None;
    let mut last = 0;
    for (i, c) in line.char_indices() {
        if let Some(q) = in_quote {
            if c == q {
                in_quote = None;
            }
        } else if c == '\'' || c == '"' {
            in_quote = Some(c);
        } else if c == ';' {
            stmts.push(&line[last..i]);
            last = i + 1;
        }
    }
    stmts.push(&line[last..]);
    stmts
}

pub(crate) fn python_imported_symbols(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
) -> Vec<String> {
    let decl_stem = decl_file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
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
                if next_line.contains(')') {
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

        for simple_stmt in split_semicolon_statements(&full_stmt) {
            let trimmed = simple_stmt.trim();
            if let Some(rest) = trimmed.strip_prefix("from ") {
                if let Some((mod_part, clause)) = rest.split_once(" import ") {
                    let clause = clause
                        .trim()
                        .trim_start_matches('(')
                        .trim_end_matches(')')
                        .trim();

                    if py_mod_matches_decl(mod_part, caller_path, decl_file) {
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
                    } else if py_mod_matches_pkg(mod_part, caller_path, decl_file) {
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
                    if py_mod_matches_decl(mod_part, caller_path, decl_file) {
                        symbols.push(fn_name.to_string());
                    }
                }
            }
        }
    }
    symbols
}
