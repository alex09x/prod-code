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

use super::decl_is_default_export;
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

fn without_line_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (i, ch) in line.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        } else if line[i..].starts_with("//") {
            return &line[..i];
        }
    }
    line
}

fn from_keyword(statement: &str) -> Option<usize> {
    let bytes = statement.as_bytes();
    let mut brace_depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active {
                quote = None;
            }
            i += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
            i += 1;
            continue;
        }
        match byte {
            b'{' => brace_depth += 1,
            b'}' => brace_depth = brace_depth.saturating_sub(1),
            _ => {}
        }
        if brace_depth == 0 && bytes[i..].starts_with(b"from") {
            let before_is_ident = i > 0
                && (bytes[i - 1].is_ascii_alphanumeric()
                    || bytes[i - 1] == b'_'
                    || bytes[i - 1] == b'$');
            let after = i + "from".len();
            let after_is_ident = bytes
                .get(after)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$');
            if !before_is_ident && !after_is_ident {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn reexport_statements(content: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut pending = String::new();
    let mut lines = content.lines().peekable();
    while let Some(raw_line) = lines.next() {
        let line = without_line_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if pending.is_empty() {
            if !line.starts_with("export") {
                continue;
            }
            pending.push_str(line);
        } else if line.starts_with("export") {
            pending.clear();
            pending.push_str(line);
        } else {
            pending.push(' ');
            pending.push_str(line);
        }
        if from_keyword(&pending).is_some() {
            statements.push(std::mem::take(&mut pending));
        } else if line.ends_with(';') {
            pending.clear();
        } else if line.ends_with('}') {
            let next_is_from = lines
                .clone()
                .map(without_line_comment)
                .map(str::trim)
                .find(|next| !next.is_empty())
                .is_some_and(|next| next.starts_with("from "));
            if !next_is_from {
                pending.clear();
            }
        }
    }
    statements
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

    for statement in reexport_statements(&content) {
        let Some(from_at) = from_keyword(&statement) else {
            continue;
        };
        let before_from = statement[..from_at].trim();
        let specifier = extract_specifier(&statement[from_at + "from".len()..]);
        if specifier.is_empty() {
            continue;
        }
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
                if source_symbol == "default"
                    && decl_is_default_export(decl_file, decl_fn).unwrap_or(false)
                {
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
