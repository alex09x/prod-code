/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;
use url::Url;

use crate::tools::execute_lsp_query;

pub(crate) async fn is_outline_definition(
    remote: SocketAddr,
    root: &Path,
    path: &Path,
    line: u32,
    character: u32,
) -> bool {
    let Ok(uri) = Url::from_file_path(path) else {
        return false;
    };
    let params = serde_json::json!({ "textDocument": { "uri": uri.to_string() } });
    let Ok(outline) =
        execute_lsp_query(remote, root, path, "textDocument/documentSymbol", params).await
    else {
        return false;
    };
    let zero_line = (line as usize).saturating_sub(1);
    let zero_col = (character as usize).saturating_sub(1);
    outline_contains_definition(&outline, zero_line, zero_col)
}

/// Returns `true` when 1-based `(line, character)` lies on a declared Rust identifier in `path`
/// (#1754), used when `textDocument/documentSymbol` returns nothing for an unindexed Rust file.
pub(crate) fn is_lexical_rust_declaration(path: &Path, line: u32, character: u32) -> bool {
    if path.extension().and_then(|e| e.to_str()) != Some("rs") {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let zero_line = (line as usize).saturating_sub(1);
    let zero_col = (character as usize).saturating_sub(1);
    use crate::tools::references::across::{mask_executable_code, masked_code_string};
    let code = masked_code_string(&text, &mask_executable_code(&text, path));
    let Some(code_line) = code.lines().nth(zero_line) else {
        return false;
    };
    crate::tools::symbols::declared_at::declared_identifiers_on_line(code_line)
        .into_iter()
        .any(|(ident, col)| zero_col >= col && zero_col < col + ident.chars().count())
}

/// Scans Rust source files under `root` (or narrowed by `hint`) for declarations of `symbol`
/// when `code_definition` resolves a Rust symbol outside the analyzer module graph (#1754).
pub(crate) fn find_unindexed_rust_declarations(
    root: &Path,
    symbol: &str,
    hint: Option<&Path>,
) -> Vec<crate::tools::symbols::SymbolHit> {
    use crate::tools::symbols::{
        SymbolHit,
        across_projects::{names_word, read_name_scan_text},
        declared_at::code_declarations,
        matching::bare_symbol_name,
        nested_projects::{MAX_SCANNED_FILES, source_files},
    };
    let (qualifier, target) = match symbol.rsplit_once("::") {
        Some((qual, name)) => (
            Some(qual.rsplit("::").next().unwrap_or(qual).trim()),
            name.trim(),
        ),
        None => (None, symbol.trim()),
    };
    let name = bare_symbol_name(target);
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Vec::new();
    }
    let files: Vec<std::path::PathBuf> = match hint {
        Some(h) => {
            let p = if h.is_absolute() {
                h.to_path_buf()
            } else {
                root.join(h)
            };
            if p.is_file() {
                if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                    vec![p]
                } else {
                    Vec::new()
                }
            } else if p.is_dir() {
                source_files(&p)
                    .filter(|f| f.extension().and_then(|e| e.to_str()) == Some("rs"))
                    .take(MAX_SCANNED_FILES)
                    .collect()
            } else {
                Vec::new()
            }
        }
        None => source_files(root)
            .filter(|f| f.extension().and_then(|e| e.to_str()) == Some("rs"))
            .take(MAX_SCANNED_FILES)
            .collect(),
    };
    let mut hits = Vec::new();
    for path in files {
        let Some(text) = read_name_scan_text(&path) else {
            continue;
        };
        if !names_word(&text, name) {
            continue;
        }
        if let Some(qual) = qualifier {
            let stem_matches = path.file_stem().and_then(|s| s.to_str()) == Some(qual);
            let parent_matches = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                == Some(qual);
            let text_matches =
                text.contains(&format!("impl {qual}")) || text.contains(&format!("mod {qual}"));
            if !stem_matches && !parent_matches && !text_matches {
                continue;
            }
        }
        for (index, col) in code_declarations(&text, &path, name) {
            hits.push(SymbolHit {
                path: path.clone(),
                name: name.to_string(),
                kind: "Declaration",
                container: qualifier.map(str::to_string),
                line: index as u32 + 1,
                col: col as u32 + 1,
            });
            if hits.len() >= 10 {
                return hits;
            }
        }
    }
    hits
}

pub(crate) fn outline_contains_definition(
    symbols: &serde_json::Value,
    line: usize,
    col: usize,
) -> bool {
    for sym in symbols.as_array().into_iter().flatten() {
        // Prefer selectionRange (the symbol's exact identifier range) so that positions
        // on `fn`, parameters or other tokens on the declaration line do not falsely match.
        let sel = sym
            .get("selectionRange")
            .or_else(|| sym.pointer("/location/range"))
            .or_else(|| sym.get("range"));
        if let Some(range) = sel
            && range_contains(range, line, col)
        {
            return true;
        }
        if let Some(children) = sym.get("children")
            && outline_contains_definition(children, line, col)
        {
            return true;
        }
    }
    false
}

pub(crate) fn range_contains(range: &serde_json::Value, line: usize, col: usize) -> bool {
    let Some(start) = range.get("start") else {
        return false;
    };
    let Some(start_line) = start
        .get("line")
        .and_then(|l| l.as_u64())
        .map(|l| l as usize)
    else {
        return false;
    };
    let start_col = start
        .get("character")
        .and_then(|c| c.as_u64())
        .map(|c| c as usize)
        .unwrap_or(0);

    let (end_line, end_col) = match range.get("end") {
        Some(end) => {
            let el = end
                .get("line")
                .and_then(|l| l.as_u64())
                .map(|l| l as usize)
                .unwrap_or(start_line);
            let ec = end
                .get("character")
                .and_then(|c| c.as_u64())
                .map(|c| c as usize)
                .unwrap_or(usize::MAX);
            (el, ec)
        }
        None => (start_line, usize::MAX),
    };

    if line < start_line || line > end_line {
        return false;
    }
    if line == start_line && col < start_col {
        return false;
    }
    if line == end_line && col >= end_col {
        return false;
    }
    true
}

/// The 0-based first and last lines of the innermost symbol of `path`'s outline whose range
/// holds the 0-based position.
pub(crate) async fn outlined_range(
    remote: SocketAddr,
    root: &Path,
    path: &Path,
    line: usize,
    col: usize,
) -> Option<(usize, usize)> {
    let uri = Url::from_file_path(path).ok()?;
    let params = serde_json::json!({ "textDocument": { "uri": uri.to_string() } });
    let outline = execute_lsp_query(remote, root, path, "textDocument/documentSymbol", params)
        .await
        .ok()?;
    let mut best: Option<Span> = None;
    innermost_holding(&outline, (line, col), &mut best);
    best.map(|(start, end)| (start.0, end.0))
}

/// An outline range: its 0-based (line, character) start and end.
type Span = ((usize, usize), (usize, usize));

/// Walks an outline, nested or flat, for the smallest range that holds `at`.
pub(crate) fn innermost_holding(
    symbols: &serde_json::Value,
    at: (usize, usize),
    best: &mut Option<Span>,
) {
    let point = |p: Option<&serde_json::Value>| -> Option<(usize, usize)> {
        let p = p?;
        Some((
            p.get("line")?.as_u64()? as usize,
            p.get("character")?.as_u64()? as usize,
        ))
    };
    for symbol in symbols.as_array().into_iter().flatten() {
        let range = symbol
            .get("range")
            .or_else(|| symbol.pointer("/location/range"));
        if let (Some(start), Some(end)) = (
            point(range.and_then(|r| r.get("start"))),
            point(range.and_then(|r| r.get("end"))),
        ) && start <= at
            && at <= end
            && best.is_none_or(|(s, e)| (end.0 - start.0, end.1) < (e.0 - s.0, e.1))
        {
            *best = Some((start, end));
        }
        if let Some(children) = symbol.get("children") {
            innermost_holding(children, at, best);
        }
    }
}
