/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashSet;
use std::path::Path;

use ra_ap_syntax::ast::HasName;
use ra_ap_syntax::{AstNode, Edition, SourceFile, SyntaxKind, TextSize, ast};

use super::incoming::unreadable;

/// Known agent and editor scratch or cache directories whose untracked contents are not project source (#760).
const SCRATCH_DIRS: &[&str] = &[
    ".prod", ".scratch", ".tmp", ".cache", ".vscode", ".idea", ".claude", ".cursor",
];

/// Whether an untracked path lies in a known scratch or cache directory (e.g. `.prod/`, `.scratch/`, `.tmp/`) (#760).
pub fn is_scratch_path(path: &str) -> bool {
    Path::new(path).components().any(|c| match c {
        std::path::Component::Normal(s) => {
            let name = s.to_string_lossy();
            SCRATCH_DIRS.iter().any(|d| name == *d)
        }
        _ => false,
    })
}

pub(crate) fn is_source_file(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|e| e.to_str()),
        Some(
            "rs" | "go"
                | "py"
                | "pyi"
                | "ts"
                | "tsx"
                | "mts"
                | "cts"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
                | "c"
                | "cc"
                | "cpp"
                | "cxx"
                | "h"
                | "hpp"
                | "hh"
                | "swift"
                | "m"
                | "mm"
                | "java"
                | "kt"
                | "kts"
                | "cs"
                | "scala"
                | "sc"
                | "zig"
        )
    )
}

/// The 1-based line or column an LSP position's 0-based `field` gives, when it is a number
/// that fits: a negative, fractional or oversized one is unreadable, not line 1.
pub fn one_based(position: &serde_json::Value, field: &str) -> Option<u32> {
    u32::try_from(position.get(field)?.as_u64()?)
        .ok()?
        .checked_add(1)
}

pub(crate) fn symbol_range(sym: &serde_json::Value) -> Option<(u32, u32, u32, u32)> {
    let range = sym
        .get("range")
        .or_else(|| sym.get("location").and_then(|l| l.get("range")))?;
    let sel = sym.get("selectionRange").unwrap_or(range);
    let start = one_based(range.get("start")?, "line")?;
    let end = one_based(range.get("end")?, "line")?;
    let sl = one_based(sel.get("start")?, "line")?;
    let sc = one_based(sel.get("start")?, "character")?;
    (start <= end).then_some((start, end, sl, sc))
}

fn code_only(text: &str) -> Option<String> {
    let lex = super::rust_attr::rust_code(text).ok()?;
    let mut bytes = text.as_bytes().to_vec();
    for (index, is_code) in lex.code.iter().enumerate() {
        if !is_code && !matches!(bytes[index], b'\n' | b'\r' | b'\t') {
            bytes[index] = b' ';
        }
    }
    String::from_utf8(bytes).ok()
}

/// The byte offset of a 1-based LSP line and UTF-16 column, when the column lies on a char
/// boundary within that line (before its line ending); otherwise unknown, never the line start.
fn symbol_byte_offset(text: &str, line_1based: u32, col_1based: u32) -> Option<usize> {
    let line_index = usize::try_from(line_1based.checked_sub(1)?).ok()?;
    let col_units = col_1based.checked_sub(1)?;
    let mut line_start = 0usize;
    for _ in 0..line_index {
        line_start += text.get(line_start..)?.find('\n')? + 1;
    }
    let rest = &text[line_start..];
    let row = rest.split('\n').next().unwrap_or(rest);
    let row = row.strip_suffix('\r').unwrap_or(row);
    let col_byte = crate::tools::symbols::sources::byte_at_utf16(row, col_units)?;
    Some(line_start + col_byte)
}

fn is_rust_local_or_field(parsed: &SourceFile, text: &str, sym: &serde_json::Value) -> bool {
    let Some((_start, _end, sl, sc)) = symbol_range(sym) else {
        return false;
    };
    let Some(offset) = symbol_byte_offset(text, sl, sc) else {
        return false;
    };
    let syntax = parsed.syntax();
    let token = syntax
        .token_at_offset(TextSize::from(offset as u32))
        .right_biased()
        .or_else(|| {
            syntax
                .token_at_offset(TextSize::from(offset as u32))
                .left_biased()
        });
    let Some(mut token) = token else {
        return false;
    };
    if token.kind().is_trivia()
        && let Some(next) = token.next_token()
    {
        token = next;
    }
    for ancestor in token.parent_ancestors() {
        match ancestor.kind() {
            SyntaxKind::RECORD_FIELD | SyntaxKind::TUPLE_FIELD => {
                return true;
            }
            SyntaxKind::LET_STMT | SyntaxKind::MATCH_ARM => {
                return true;
            }
            SyntaxKind::CLOSURE_EXPR => {
                return true;
            }
            SyntaxKind::FN => {
                if let Some(func) = ast::Fn::cast(ancestor)
                    && let Some(name) = func.name()
                    && name
                        .syntax()
                        .text_range()
                        .contains_range(token.text_range())
                {
                    return false;
                }
                return true;
            }
            _ => {}
        }
    }
    false
}

pub(crate) fn is_callable_symbol(kind: u64, sym: &serde_json::Value, text: Option<&str>) -> bool {
    if matches!(kind, 6 | 9 | 12) {
        return true;
    }
    if matches!(kind, 7 | 8 | 13 | 14) {
        if let Some(detail) = sym.get("detail").and_then(|d| d.as_str())
            && (detail.contains("=>")
                || detail.contains("function")
                || detail.starts_with("fn(")
                || detail.contains(" Fn(")
                || detail.contains(" FnMut(")
                || detail.contains(" FnOnce("))
        {
            return true;
        }
        if let Some(text) = text
            && let Some((start, end, _, _)) = symbol_range(sym)
            && start > 0
        {
            let lines: Vec<&str> = text.lines().collect();
            let idx = (start - 1) as usize;
            let decl = lines.get(idx).copied().unwrap_or("").trim();
            if decl.contains("match ")
                || decl.starts_with("if let ")
                || decl.starts_with("while let ")
            {
                return false;
            }
            let end = (end as usize).min(lines.len());
            for line in lines.get(idx..end).unwrap_or_default() {
                let trimmed = line.trim();
                let is_fn_kw = trimmed.starts_with("function ")
                    || trimmed.starts_with("function(")
                    || trimmed.starts_with("function*")
                    || trimmed.contains(" function ")
                    || trimmed.contains(" function(")
                    || trimmed.contains(" function*")
                    || trimmed.contains("async function")
                    || trimmed.contains("= function")
                    || trimmed.contains(": function");
                if is_fn_kw || trimmed.contains("async (") {
                    return true;
                }
                if let Some(arrow_pos) = trimmed.find("=>")
                    && trimmed[..arrow_pos].contains('=')
                    && !trimmed[..arrow_pos].contains("match ")
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Functions and methods (LSP kinds 6, 9, 12, and callable properties/variables) in a document,
/// flattened with their ranges. An entry that is not a symbol (no name or kind, children that
/// are not a list, a function without a readable range) is an error: skipped, it would hide a
/// changed function.
pub(crate) fn collect_functions(
    symbols: &[serde_json::Value],
    text: Option<&str>,
    language: &str,
    out: &mut Vec<(String, u32, u32, u32, u32)>,
) -> std::result::Result<(), String> {
    let code = text.and_then(code_only);
    let mut function_names: HashSet<String> = symbols
        .iter()
        .filter(|symbol| {
            matches!(
                symbol.get("kind").and_then(|kind| kind.as_u64()),
                Some(6 | 9 | 12)
            )
        })
        .filter_map(|symbol| symbol.get("name").and_then(|name| name.as_str()))
        .map(str::to_string)
        .collect();
    for symbol in symbols {
        if let Some(kind) = symbol.get("kind").and_then(|k| k.as_u64())
            && matches!(kind, 7 | 8 | 13 | 14)
            && let Some(container) = symbol.get("containerName").and_then(|c| c.as_str())
            && function_names.contains(container)
            && let Some(name) = symbol.get("name").and_then(|n| n.as_str())
        {
            function_names.insert(name.to_string());
        }
    }
    // Only a Rust file gets Rust AST classification: an error-recovered Rust tree of Go or
    // JavaScript text turns names such as `fn` into keyword nodes and drops real functions.
    let parsed = text.and_then(|t| {
        (language == "rust" && code.is_some()).then(|| SourceFile::parse(t, Edition::CURRENT))
    });
    collect_functions_inner(
        symbols,
        code.as_deref().or(text),
        text,
        parsed.as_ref().map(|p| p.tree()).as_ref(),
        false,
        &function_names,
        out,
    )
}

fn collect_functions_inner(
    symbols: &[serde_json::Value],
    code_text: Option<&str>,
    raw_text: Option<&str>,
    parsed: Option<&SourceFile>,
    inside_callable: bool,
    function_names: &HashSet<String>,
    out: &mut Vec<(String, u32, u32, u32, u32)>,
) -> std::result::Result<(), String> {
    for sym in symbols {
        let malformed = || unreadable("textDocument/documentSymbol", sym);
        let (Some(name), Some(kind)) = (
            sym.get("name").and_then(|n| n.as_str()),
            sym.get("kind").and_then(|k| k.as_u64()),
        ) else {
            return Err(malformed());
        };
        if name.is_empty() || !(1..=26).contains(&kind) {
            return Err(malformed());
        }
        let rust_local_or_field = match (parsed, raw_text) {
            (Some(tree), Some(text)) => is_rust_local_or_field(tree, text, sym),
            _ => false,
        };
        let local_variable = rust_local_or_field
            || (kind == 13
                && sym
                    .get("containerName")
                    .and_then(|container| container.as_str())
                    .is_some_and(|container| function_names.contains(container)));
        let callable = if rust_local_or_field {
            false
        } else if inside_callable || local_variable {
            matches!(kind, 6 | 9 | 12)
        } else {
            is_callable_symbol(kind, sym, code_text)
        };
        if callable && !name.is_empty() {
            let (start, end, sl, sc) = symbol_range(sym).ok_or_else(malformed)?;
            out.push((name.to_string(), start, end, sl, sc));
        }
        match sym.get("children") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(children)) => {
                collect_functions_inner(
                    children,
                    code_text,
                    raw_text,
                    parsed,
                    inside_callable || callable,
                    function_names,
                    out,
                )?;
            }
            Some(_) => return Err(malformed()),
        }
    }
    Ok(())
}
