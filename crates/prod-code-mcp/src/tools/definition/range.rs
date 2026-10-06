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
