/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

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
    decl_stem: &str,
    fn_name: &str,
) -> Vec<String> {
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
    }
    symbols
}
